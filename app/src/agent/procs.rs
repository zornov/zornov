// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The programs the AI starts (Codex, Claude Code and what they start in
//! turn), so that Stop can end every one of them, children included.
//!
//! Each program gets its own process group, and is listed under an owner (a
//! window's AI: its [`Gate`](super::brain::Gate) and its tool worker share
//! one). `kill_owner` ends the whole tree of every program of that owner at
//! once, which killing the direct child alone does not: Codex starts shells,
//! tool servers and renders of its own, and those used to live on.

use std::cell::Cell;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
static LIVE: Mutex<Vec<(u64, u32)>> = Mutex::new(Vec::new());

thread_local! {
    static OWNER: Cell<u64> = const { Cell::new(0) };
}

/// A new owner id.
pub fn new_owner() -> u64 {
    NEXT_OWNER.fetch_add(1, Ordering::SeqCst)
}

/// The owner of programs this thread starts (0: none in particular).
pub fn owner() -> u64 {
    OWNER.with(Cell::get)
}

/// Makes `id` the owner of what this thread starts, and of the threads it
/// hands the id to.
pub fn set_owner(id: u64) {
    OWNER.with(|o| o.set(id));
}

fn live() -> std::sync::MutexGuard<'static, Vec<(u64, u32)>> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The variable every program the AI starts carries, with the pid of the
/// Studio that started it. A later Studio finds the ones whose Studio is
/// gone by it (see [`reap_orphans`]) and never touches any other program,
/// the person's own Codex sessions included.
pub const MARKER: &str = "STUDIO_AI_CHILD";

/// Starts `cmd` in a process group of its own, listed under `owner`, marked
/// as Studio's, and watched: if Studio itself ends (quit, crash, kill), the
/// group is ended too.
pub fn spawn_as(owner: u64, cmd: &mut Command) -> std::io::Result<Child> {
    spawn_for(owner, std::process::id(), cmd)
}

/// [`spawn_as`] for this thread's owner.
pub fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    spawn_as(owner(), cmd)
}

fn spawn_for(owner: u64, parent: u32, cmd: &mut Command) -> std::io::Result<Child> {
    cmd.env(MARKER, parent.to_string());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    // Zornov is a window program on Windows: without this, every console
    // program it starts would open a console window of its own.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let child = cmd.spawn()?;
    live().push((owner, child.id()));
    watch(parent, child.id());
    Ok(child)
}

/// A small watcher beside the group `pgid`: when `parent` is gone it ends
/// the group (no handler can run in a Studio that was killed); when the
/// group is gone first it just ends.
fn watch(parent: u32, pgid: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        let script = format!("while kill -0 {parent} 2>/dev/null && kill -0 -- -{pgid} 2>/dev/null; do sleep 1; done; kill -0 {parent} 2>/dev/null || kill -KILL -- -{pgid} 2>/dev/null");
        let _ = Command::new("/bin/sh").args(["-c", &script]).process_group(0).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|mut w| {
            // Reaped once it ends, without holding anything up.
            std::thread::spawn(move || {
                let _ = w.wait();
            });
        });
    }
    #[cfg(not(unix))]
    let _ = (parent, pgid);
}

/// Studio is quitting: every program the AI started ends, with its children.
pub fn kill_all() -> usize {
    let pids: Vec<u32> = std::mem::take(&mut *live()).into_iter().map(|(_, p)| p).collect();
    for p in &pids {
        kill_tree(*p);
    }
    pids.len()
}

/// At launch: ends the programs a previous Studio started and left behind
/// (found by [`MARKER`], and only when the Studio named in it is gone).
/// Returns how many.
pub fn reap_orphans() -> usize {
    #[cfg(unix)]
    {
        let me = std::process::id();
        let mut n = 0;
        for (pid, owner) in marked() {
            if owner != me && pid != me && !alive_any(owner) {
                kill_tree(pid);
                n += 1;
            }
        }
        n
    }
    #[cfg(not(unix))]
    0
}

/// Whether process `pid` exists at all.
#[cfg(unix)]
fn alive_any(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as i32, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) }
}

/// The environment of process `pid` (empty when it cannot be read).
#[cfg(target_os = "macos")]
fn environment(pid: u32) -> Vec<String> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut size: libc::size_t = 0;
    // SAFETY: the standard two-step sysctl read of a process' arguments.
    unsafe {
        if libc::sysctl(mib.as_mut_ptr(), 3, std::ptr::null_mut(), &mut size, std::ptr::null_mut(), 0) != 0 || size < 4 {
            return Vec::new();
        }
        let mut buf = vec![0u8; size];
        if libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, std::ptr::null_mut(), 0) != 0 {
            return Vec::new();
        }
        buf.truncate(size);
        let argc = i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]).max(0) as usize;
        // After argc: the program path, nul padding, argc arguments, then the environment.
        let mut parts = buf[4..].split(|b| *b == 0);
        let mut skipped = 0;
        let mut env = Vec::new();
        let mut started = false;
        // The path first.
        let _ = parts.next();
        for p in parts {
            if !started {
                if p.is_empty() {
                    continue;
                }
                skipped += 1;
                if skipped > argc {
                    started = true;
                } else {
                    continue;
                }
            }
            if p.is_empty() {
                break;
            }
            env.push(String::from_utf8_lossy(p).into_owned());
        }
        env
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn environment(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/environ")).map(|b| b.split(|c| *c == 0).map(|p| String::from_utf8_lossy(p).into_owned()).collect()).unwrap_or_default()
}

/// Every (pid, Studio pid) of a program that carries the marker.
#[cfg(unix)]
fn marked() -> Vec<(u32, u32)> {
    let key = format!("{MARKER}=");
    table()
        .into_iter()
        .filter_map(|(pid, _)| {
            let owner = environment(pid).into_iter().find_map(|e| e.strip_prefix(&key).and_then(|v| v.parse::<u32>().ok()))?;
            Some((pid, owner))
        })
        .collect()
}

/// The program ended and was waited for: no longer listed.
pub fn release(pid: u32) {
    live().retain(|(_, p)| *p != pid);
}

/// Whether process `pid` still exists (a zombie that is not waited for
/// counts as gone).
pub fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process exists.
        let exists = unsafe { libc::kill(pid as i32, 0) } == 0;
        exists && !is_zombie(pid)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(unix)]
fn is_zombie(pid: u32) -> bool {
    Command::new("ps").args(["-o", "stat=", "-p", &pid.to_string()]).output().ok().is_some_and(|o| String::from_utf8_lossy(&o.stdout).trim_start().starts_with('Z'))
}

/// Every (pid, parent) on the machine.
#[cfg(unix)]
fn table() -> Vec<(u32, u32)> {
    let Ok(out) = Command::new("ps").args(["-A", "-o", "pid=,ppid="]).output() else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
        })
        .collect()
}

/// Ends `pid`, its process group and everything below it, however deep and
/// whichever group they moved to.
pub fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        // Found before anything dies: a child orphaned by its parent's end
        // would no longer lead here.
        let t = table();
        let mut all = vec![pid];
        let mut i = 0;
        while i < all.len() {
            let p = all[i];
            let kids: Vec<u32> = t.iter().filter(|(c, pp)| *pp == p && !all.contains(c)).map(|(c, _)| *c).collect();
            all.extend(kids);
            i += 1;
        }
        // Stopped first so that nothing starts another child meanwhile.
        for sig in [libc::SIGSTOP, libc::SIGKILL] {
            // SAFETY: plain signals to processes this module started.
            unsafe {
                libc::kill(-(pid as i32), sig);
                for p in &all {
                    libc::kill(*p as i32, sig);
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = Command::new("taskkill").args(["/F", "/T", "/PID", &pid.to_string()]).creation_flags(CREATE_NO_WINDOW).output();
    }
}

/// Ends every program of `owner`, with their children. Returns how many.
pub fn kill_owner(owner: u64) -> usize {
    let pids: Vec<u32> = {
        let mut l = live();
        let mine = l.iter().filter(|(o, _)| *o == owner).map(|(_, p)| *p).collect();
        l.retain(|(o, _)| *o != owner);
        mine
    };
    for p in &pids {
        kill_tree(*p);
    }
    pids.len()
}

/// How many programs `owner` has listed.
pub fn count(owner: u64) -> usize {
    live().iter().filter(|(o, _)| *o == owner).count()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn wait_gone(pids: &[u32], secs: u64) -> bool {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        while pids.iter().any(|p| alive(*p)) {
            if std::time::Instant::now() > end {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        true
    }

    /// A program with a child of its own; returns it and the grandchild's pid.
    fn family(pidfile: &std::path::Path) -> Command {
        let mut c = Command::new("/bin/sh");
        c.args(["-c", &format!("sleep 703 & echo $! > '{}'; wait", pidfile.display())]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        c
    }

    fn grandchild(pidfile: &std::path::Path) -> u32 {
        for _ in 0..100 {
            if let Some(p) = std::fs::read_to_string(pidfile).ok().and_then(|t| t.trim().parse().ok()) {
                return p;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("no child started");
    }

    #[test]
    fn quitting_ends_every_program_and_its_children() {
        let dir = std::env::temp_dir().join(format!("studio-procs-quit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let owner = new_owner();
        let mut all = Vec::new();
        for i in 0..3 {
            let f = dir.join(format!("g{i}"));
            let child = spawn_as(owner, &mut family(&f)).unwrap();
            all.push(child.id());
            all.push(grandchild(&f));
        }
        assert!(all.iter().all(|p| alive(*p)));
        assert!(kill_all() >= 3);
        assert!(wait_gone(&all, 10), "programs survived the quit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_killed_studio_takes_its_programs_with_it() {
        let dir = std::env::temp_dir().join(format!("studio-procs-kill-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A stand-in for the Studio that gets killed.
        let mut studio = Command::new("sleep").arg("704").spawn().unwrap();
        let f = dir.join("g");
        let child = spawn_for(new_owner(), studio.id(), &mut family(&f)).unwrap();
        let all = [child.id(), grandchild(&f)];
        assert!(all.iter().all(|p| alive(*p)));
        studio.kill().unwrap();
        let _ = studio.wait();
        assert!(wait_gone(&all, 15), "programs outlived their Zornov");
        release(child.id());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The program the reaper test starts (the system's own programs hide
    /// their environment, so the test binary stands in for Codex).
    #[test]
    #[ignore = "a stand-in program for launch_reaps_only_the_orphans_of_a_dead_studio"]
    fn sleeper() {
        std::thread::sleep(std::time::Duration::from_secs(120));
    }

    fn sleeper_cmd() -> Command {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args(["--ignored", "--exact", "agent::procs::tests::sleeper"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        c
    }

    fn wait_marked(pid: u32) {
        for _ in 0..200 {
            if environment(pid).iter().any(|e| e.starts_with(MARKER) || e.starts_with("PATH=")) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the environment of {pid} cannot be read");
    }

    #[test]
    fn launch_reaps_only_the_orphans_of_a_dead_studio() {
        // A pid that is certainly gone.
        let mut gone = Command::new("true").spawn().unwrap();
        let dead = gone.id();
        let _ = gone.wait();
        // Left by the dead Studio.
        let mut orphan = sleeper_cmd();
        orphan.env(MARKER, dead.to_string());
        let mut orphan = orphan.spawn().unwrap();
        // Belongs to this (living) Studio.
        let mut mine = sleeper_cmd();
        mine.env(MARKER, std::process::id().to_string());
        let mut mine = mine.spawn().unwrap();
        // The person's own program: no marker.
        let mut theirs = sleeper_cmd();
        theirs.env_remove(MARKER);
        let mut theirs = theirs.spawn().unwrap();
        for p in [&orphan, &mine, &theirs] {
            wait_marked(p.id());
        }
        assert!(reap_orphans() >= 1);
        let _ = orphan.wait();
        assert!(!alive(orphan.id()), "the orphan survived");
        assert!(alive(mine.id()) && alive(theirs.id()), "something that was not an orphan was ended");
        for c in [&mut mine, &mut theirs] {
            kill_tree(c.id());
            let _ = c.wait();
        }
    }
}
