// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The sandbox: plugins run as WebAssembly in wasmtime.
//!
//! A plugin can import exactly one host function, `studio.log`. It has no
//! WASI, so no files, sockets, clock, randomness or environment. Each
//! instance has a memory cap ([`Limits::memory`]) and each call a time
//! limit ([`Limits::call`]); a plugin that traps, runs over time or asks
//! for too much memory fails that call only, and the picture is left as it
//! was. Instances are pooled so several render threads can use one plugin
//! at once.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use wasmtime::{Config, Engine, Instance, Linker, Memory, Module, Store, StoreLimits, StoreLimitsBuilder, TypedFunc};

use crate::manifest::Manifest;

/// What a plugin may use.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Most linear memory one instance may grow to, in bytes.
    pub memory: usize,
    /// Longest one call may run.
    pub call: Duration,
    /// Most instances of one plugin alive at once.
    pub instances: usize,
}

impl Default for Limits {
    fn default() -> Self {
        // A 4K transition needs two 133 MB frames in the guest.
        Limits { memory: 1536 << 20, call: Duration::from_secs(20), instances: 4 }
    }
}

/// Epoch ticks per second (see [`engine`]).
const TICKS_PER_SEC: u64 = 20;

/// The shared engine. A background thread advances its epoch so calls can
/// be stopped when they run over time.
pub fn engine() -> &'static Engine {
    static E: OnceLock<Engine> = OnceLock::new();
    E.get_or_init(|| {
        let mut c = Config::new();
        c.epoch_interruption(true);
        c.cranelift_opt_level(wasmtime::OptLevel::Speed);
        let e = Engine::new(&c).expect("wasmtime engine");
        let weak = e.weak();
        std::thread::Builder::new()
            .name("plugin-epoch".into())
            .spawn(move || {
                while let Some(e) = weak.upgrade() {
                    e.increment_epoch();
                    drop(e);
                    std::thread::sleep(Duration::from_millis(1000 / TICKS_PER_SEC));
                }
            })
            .ok();
        e
    })
}

struct HostState {
    limits: StoreLimits,
    log: Arc<Mutex<Vec<String>>>,
}

struct Inst {
    store: Store<HostState>,
    instance: Instance,
    memory: Memory,
    alloc: TypedFunc<u32, u32>,
    free: TypedFunc<(u32, u32), ()>,
}

/// A compiled plugin and its pool of instances.
pub struct Runtime {
    module: Module,
    limits: Limits,
    pool: Mutex<Vec<Inst>>,
    live: AtomicU32,
    /// Lines the plugin logged and calls that failed, newest last.
    pub log: Arc<Mutex<Vec<String>>>,
    /// Calls that trapped or ran over time since loading.
    pub faults: AtomicU32,
}

/// What `studio_manifest` and the other calls can fail with.
pub type CallResult<T> = Result<T, String>;

impl Runtime {
    /// Compiles a module and checks its exports. Nothing of the plugin runs
    /// yet.
    pub fn compile(wasm: &[u8], limits: Limits) -> CallResult<Runtime> {
        let module = Module::new(engine(), wasm).map_err(|e| format!("not a WebAssembly plugin: {e:#}"))?;
        for imp in module.imports() {
            if (imp.module(), imp.name()) != ("studio", "log") {
                return Err(format!(
                    "the plugin asks for `{}.{}`, which Studio does not give plugins (plugins may only import studio.log; build for wasm32-unknown-unknown, not WASI)",
                    imp.module(),
                    imp.name()
                ));
            }
        }
        for need in ["memory", "studio_abi", "studio_alloc", "studio_free", "studio_manifest"] {
            if module.get_export(need).is_none() {
                return Err(format!("the plugin does not export `{need}`; build it with studio-plugin-api's export_plugin!"));
            }
        }
        Ok(Runtime { module, limits, pool: Mutex::new(Vec::new()), live: AtomicU32::new(0), log: Arc::new(Mutex::new(Vec::new())), faults: AtomicU32::new(0) })
    }

    fn instantiate(&self) -> CallResult<Inst> {
        let state = HostState { limits: StoreLimitsBuilder::new().memory_size(self.limits.memory).instances(1).tables(4).memories(1).trap_on_grow_failure(true).build(), log: self.log.clone() };
        let mut store = Store::new(engine(), state);
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(self.ticks());
        store.epoch_deadline_trap();
        let mut linker: Linker<HostState> = Linker::new(engine());
        linker
            .func_wrap("studio", "log", |mut caller: wasmtime::Caller<'_, HostState>, ptr: u32, len: u32| {
                let Some(mem) = caller.get_export("memory").and_then(|e| e.into_memory()) else { return };
                let mut buf = vec![0u8; (len as usize).min(4096)];
                if mem.read(&caller, ptr as usize, &mut buf).is_ok() {
                    let line = String::from_utf8_lossy(&buf).into_owned();
                    let mut log = caller.data().log.lock().unwrap_or_else(|e| e.into_inner());
                    push_line(&mut log, line);
                }
            })
            .map_err(|e| e.to_string())?;
        let instance = linker.instantiate(&mut store, &self.module).map_err(|e| format!("the plugin failed to start: {e:#}"))?;
        let memory = instance.get_memory(&mut store, "memory").ok_or("the plugin has no memory")?;
        let alloc = instance.get_typed_func::<u32, u32>(&mut store, "studio_alloc").map_err(|e| e.to_string())?;
        let free = instance.get_typed_func::<(u32, u32), ()>(&mut store, "studio_free").map_err(|e| e.to_string())?;
        let abi = instance.get_typed_func::<(), u32>(&mut store, "studio_abi").map_err(|e| e.to_string())?.call(&mut store, ()).map_err(|e| e.to_string())?;
        if !crate::manifest::SUPPORTED_ABI.contains(&abi) {
            return Err(format!("the plugin speaks plugin ABI {abi}, which this Studio does not"));
        }
        Ok(Inst { store, instance, memory, alloc, free })
    }

    fn ticks(&self) -> u64 {
        (self.limits.call.as_millis() as u64 * TICKS_PER_SEC / 1000).max(1)
    }

    /// Runs `f` on a pooled instance (making one when all are busy). An
    /// instance whose call failed is thrown away: a trap can leave its
    /// memory in any state.
    fn with<T>(&self, what: &str, f: impl FnOnce(&mut Inst) -> CallResult<T>) -> CallResult<T> {
        let pooled = self.pool.lock().unwrap_or_else(|e| e.into_inner()).pop();
        let mut inst = match pooled {
            Some(i) => i,
            None => {
                // Over the cap, wait for one to come back.
                if self.live.load(Ordering::Relaxed) as usize >= self.limits.instances {
                    let mut tries = 0;
                    loop {
                        if let Some(i) = self.pool.lock().unwrap_or_else(|e| e.into_inner()).pop() {
                            break i;
                        }
                        tries += 1;
                        if tries > 20 * 60 || (self.live.load(Ordering::Relaxed) as usize) < self.limits.instances {
                            self.live.fetch_add(1, Ordering::Relaxed);
                            break self.instantiate().inspect_err(|_| {
                                self.live.fetch_sub(1, Ordering::Relaxed);
                            })?;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                } else {
                    self.live.fetch_add(1, Ordering::Relaxed);
                    self.instantiate().inspect_err(|_| {
                        self.live.fetch_sub(1, Ordering::Relaxed);
                    })?
                }
            }
        };
        inst.store.set_epoch_deadline(self.ticks());
        match f(&mut inst) {
            Ok(v) => {
                self.pool.lock().unwrap_or_else(|e| e.into_inner()).push(inst);
                Ok(v)
            }
            Err(e) => {
                self.live.fetch_sub(1, Ordering::Relaxed);
                self.faults.fetch_add(1, Ordering::Relaxed);
                let msg = format!("{what} failed: {e}");
                push_line(&mut self.log.lock().unwrap_or_else(|e| e.into_inner()), msg.clone());
                Err(msg)
            }
        }
    }

    /// The plugin's manifest.
    pub fn manifest(&self) -> CallResult<Manifest> {
        let bytes = self.with("studio_manifest", |i| {
            let f = i.instance.get_typed_func::<(), u64>(&mut i.store, "studio_manifest").map_err(|e| e.to_string())?;
            let packed = f.call(&mut i.store, ()).map_err(trap)?;
            i.take(packed)
        })?;
        Manifest::parse(&String::from_utf8_lossy(&bytes))
    }

    /// Runs `studio_effect` or `studio_generate` on `pixels` in place.
    pub fn frame_call(&self, export: &str, index: u32, ctx: [u32; 4], pixels: &mut [[f32; 4]], width: u32, height: u32, packed: &[[f32; 4]]) -> CallResult<()> {
        self.with(export, |i| {
            let f = i.instance.get_typed_func::<(u32, u32, u32, u32, u32, u32, u32), i32>(&mut i.store, export).map_err(|e| e.to_string())?;
            let c = i.put(&words(&ctx))?;
            let px = i.put(as_bytes(pixels))?;
            let p = i.put(as_bytes(packed))?;
            let status = f.call(&mut i.store, (index, c.0, px.0, width, height, p.0, packed.len() as u32)).map_err(trap)?;
            if status != 0 {
                return Err(format!("the plugin reported an error ({status})"));
            }
            i.read(px.0, as_bytes_mut(pixels))?;
            i.release(&[c, px, p])
        })
    }

    /// Runs `studio_transition`: `a` becomes the blend.
    #[allow(clippy::too_many_arguments)]
    pub fn transition_call(&self, index: u32, ctx: [u32; 4], a: &mut [[f32; 4]], b: &[[f32; 4]], width: u32, height: u32, packed: &[[f32; 4]]) -> CallResult<()> {
        self.with("studio_transition", |i| {
            let f = i.instance.get_typed_func::<(u32, u32, u32, u32, u32, u32, u32, u32), i32>(&mut i.store, "studio_transition").map_err(|e| e.to_string())?;
            let c = i.put(&words(&ctx))?;
            let pa = i.put(as_bytes(a))?;
            let pb = i.put(as_bytes(b))?;
            let p = i.put(as_bytes(packed))?;
            let status = f.call(&mut i.store, (index, c.0, pa.0, pb.0, width, height, p.0, packed.len() as u32)).map_err(trap)?;
            if status != 0 {
                return Err(format!("the plugin reported an error ({status})"));
            }
            i.read(pa.0, as_bytes_mut(a))?;
            i.release(&[c, pa, pb, p])
        })
    }

    /// Runs a call that takes byte strings and returns a byte string
    /// (`studio_import`, `studio_export`, `studio_panel`).
    pub fn bytes_call(&self, export: &str, index: u32, args: &[&[u8]]) -> CallResult<Vec<u8>> {
        self.with(export, |i| {
            let mut put = Vec::new();
            for a in args {
                put.push(i.put(a)?);
            }
            let packed = match args.len() {
                1 => {
                    let f = i.instance.get_typed_func::<(u32, u32, u32), u64>(&mut i.store, export).map_err(|e| e.to_string())?;
                    f.call(&mut i.store, (index, put[0].0, put[0].1)).map_err(trap)?
                }
                2 => {
                    let f = i.instance.get_typed_func::<(u32, u32, u32, u32, u32), u64>(&mut i.store, export).map_err(|e| e.to_string())?;
                    f.call(&mut i.store, (index, put[0].0, put[0].1, put[1].0, put[1].1)).map_err(trap)?
                }
                n => return Err(format!("{n} arguments")),
            };
            let out = i.take(packed)?;
            i.release(&put)?;
            Ok(out)
        })
    }

    /// Whether the module exports `name` (older plugins may lack a call).
    pub fn exports(&self, name: &str) -> bool {
        self.module.get_export(name).is_some()
    }
}

impl Inst {
    /// Copies bytes into a fresh guest block: (pointer, length).
    fn put(&mut self, bytes: &[u8]) -> CallResult<(u32, u32)> {
        let n = u32::try_from(bytes.len()).map_err(|_| "too much data for a plugin")?;
        let p = self.alloc.call(&mut self.store, n).map_err(trap)?;
        if p == 0 && n > 0 {
            return Err("the plugin is out of memory".into());
        }
        self.memory.write(&mut self.store, p as usize, bytes).map_err(|_| "the plugin returned a bad pointer".to_owned())?;
        Ok((p, n))
    }

    fn read(&self, ptr: u32, out: &mut [u8]) -> CallResult<()> {
        self.memory.read(&self.store, ptr as usize, out).map_err(|_| "the plugin returned a bad pointer".to_owned())
    }

    fn release(&mut self, blocks: &[(u32, u32)]) -> CallResult<()> {
        for &(p, n) in blocks {
            self.free.call(&mut self.store, (p, n)).map_err(trap)?;
        }
        Ok(())
    }

    /// Reads and frees a `ptr << 32 | len` result.
    fn take(&mut self, packed: u64) -> CallResult<Vec<u8>> {
        let (ptr, len) = ((packed >> 32) as u32, packed as u32);
        if len as usize > 512 << 20 {
            return Err("the plugin returned too much data".into());
        }
        let mut out = vec![0u8; len as usize];
        self.read(ptr, &mut out)?;
        self.release(&[(ptr, len)])?;
        Ok(out)
    }
}

fn push_line(log: &mut Vec<String>, line: String) {
    log.push(line);
    if log.len() > 200 {
        log.drain(..log.len() - 200);
    }
}

fn trap(e: wasmtime::Error) -> String {
    if let Some(t) = e.downcast_ref::<wasmtime::Trap>() {
        return match t {
            wasmtime::Trap::Interrupt => "the plugin ran over its time limit".to_owned(),
            wasmtime::Trap::UnreachableCodeReached => "the plugin crashed (it panicked)".to_owned(),
            wasmtime::Trap::MemoryOutOfBounds => "the plugin read outside its memory".to_owned(),
            other => format!("the plugin crashed ({other})"),
        };
    }
    let s = format!("{e:#}");
    if s.contains("memory") && s.contains("limit") { "the plugin asked for more memory than it may use".to_owned() } else { s }
}

fn words(w: &[u32; 4]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, x) in w.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_le_bytes());
    }
    out
}

fn as_bytes(px: &[[f32; 4]]) -> &[u8] {
    // SAFETY: `[f32; 4]` has no padding and any byte pattern is a valid u8;
    // wasm and every host Studio runs on are little-endian.
    unsafe { std::slice::from_raw_parts(px.as_ptr() as *const u8, std::mem::size_of_val(px)) }
}

fn as_bytes_mut(px: &mut [[f32; 4]]) -> &mut [u8] {
    // SAFETY: as above, and every byte pattern is a valid f32.
    unsafe { std::slice::from_raw_parts_mut(px.as_mut_ptr() as *mut u8, std::mem::size_of_val(px)) }
}

/// Splits `[u32 header length][header][payload]`.
pub fn split_blob(b: &[u8]) -> CallResult<(serde_json::Value, &[u8])> {
    let n = u32::from_le_bytes(b.get(..4).ok_or("the plugin returned nothing")?.try_into().unwrap_or([0; 4])) as usize;
    let head = b.get(4..4 + n).ok_or("the plugin returned a short reply")?;
    let v = serde_json::from_slice(head).map_err(|e| format!("the plugin returned bad JSON: {e}"))?;
    Ok((v, &b[4 + n..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(wat: &str, limits: Limits) -> CallResult<Runtime> {
        // wasmtime reads the text format too.
        Runtime::compile(wat.as_bytes(), limits)
    }

    const SHELL: &str = r#"
        (memory (export "memory") 1 1000)
        (global $top (mut i32) (i32.const 1024))
        (func (export "studio_abi") (result i32) i32.const 1)
        (func (export "studio_alloc") (param i32) (result i32)
            (local $p i32)
            global.get $top local.set $p
            global.get $top local.get 0 i32.add i32.const 15 i32.add i32.const -16 i32.and global.set $top
            ;; grow when needed
            (block $ok
                global.get $top memory.size i32.const 65536 i32.mul i32.le_u br_if $ok
                global.get $top memory.size i32.const 65536 i32.mul i32.sub i32.const 65535 i32.add i32.const 65536 i32.div_u memory.grow drop)
            local.get $p)
        (func (export "studio_free") (param i32 i32))
    "#;

    #[test]
    fn sandbox_refuses_wasi_and_stops_runaway_and_greedy_plugins() {
        // Asking for WASI is refused before anything runs.
        let wasi = r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32))) (memory (export "memory") 1))"#;
        assert!(compile(wasi, Limits::default()).err().unwrap().contains("wasi_snapshot_preview1.fd_write"));

        let spin = format!(
            r#"(module {SHELL}
                (func (export "studio_manifest") (result i64) i64.const 0)
                (func (export "studio_effect") (param i32 i32 i32 i32 i32 i32 i32) (result i32) (loop $l br $l) i32.const 0))"#
        );
        let rt = compile(&spin, Limits { call: Duration::from_millis(300), ..Limits::default() }).unwrap();
        let mut px = vec![[0.5f32; 4]; 4];
        let t = std::time::Instant::now();
        let e = rt.frame_call("studio_effect", 0, [0; 4], &mut px, 2, 2, &[]).unwrap_err();
        assert!(e.contains("time limit"), "{e}");
        assert!(t.elapsed() < Duration::from_secs(5));
        assert_eq!(px[0], [0.5; 4], "a failed call leaves the picture alone");
        assert_eq!(rt.faults.load(Ordering::Relaxed), 1);

        // Allocating past the memory cap fails the call, not the host.
        let greedy = format!(
            r#"(module {SHELL}
                (func (export "studio_manifest") (result i64) i64.const 0)
                (func (export "studio_effect") (param i32 i32 i32 i32 i32 i32 i32) (result i32)
                    i32.const 2000 memory.grow i32.const -1 i32.eq (if (then unreachable)) i32.const 0))"#
        );
        let rt = compile(&greedy, Limits { memory: 16 << 20, ..Limits::default() }).unwrap();
        assert!(rt.frame_call("studio_effect", 0, [0; 4], &mut px, 2, 2, &[]).is_err());
    }
}
