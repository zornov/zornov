// Kernel / backend benchmarks: `cargo run -p studio-nn --release --example bench`.
//
// Every figure is the median of N timed runs after warm-up. Metal timings
// include host↔GPU transfers of activations (constant weights are cached on
// the GPU, as in a session).

use studio_nn::audio::{Fft, whisper_log_mel};
use studio_nn::backend::{Backend, BackendKind, metal_available};
use studio_nn::kernels::gemm::sgemm;
use studio_nn::onnx::{GraphBuilder, Node, attr_i, attr_ints};
use studio_nn::ops::{OpCtx, nn, prepack};
use studio_nn::{DType, Options, Session, Tensor};
use std::time::Instant;

#[cfg(unix)]
fn load_average() -> Option<f64> {
    unsafe extern "C" {
        fn getloadavg(loadavg: *mut f64, n: i32) -> i32;
    }
    let mut l = [0.0f64; 1];
    (unsafe { getloadavg(l.as_mut_ptr(), 1) } == 1).then_some(l[0])
}

#[cfg(not(unix))]
fn load_average() -> Option<f64> {
    None
}

fn median_secs(warm: usize, runs: usize, mut f: impl FnMut()) -> f64 {
    for _ in 0..warm {
        f();
    }
    let mut t: Vec<f64> = (0..runs)
        .map(|_| {
            let t0 = Instant::now();
            f();
            t0.elapsed().as_secs_f64()
        })
        .collect();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    t[t.len() / 2]
}

fn rand_vec(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((s >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
        })
        .collect()
}

fn row(name: &str, backend: &str, secs: Option<f64>, flops: Option<f64>) {
    match secs {
        Some(s) => {
            let gf = flops.map(|f| format!("{:>10.1}", f / s / 1e9)).unwrap_or_else(|| format!("{:>10}", "-"));
            let t = if s < 1e-3 { format!("{:>9.2} us", s * 1e6) } else { format!("{:>9.3} ms", s * 1e3) };
            println!("| {name:<34} | {backend:<6} | {t} | {gf} |");
        }
        None => println!("| {name:<34} | {backend:<6} | {:>12} | {:>10} |", "n/a", "-"),
    }
}

struct Gpu {
    be: Option<Box<dyn Backend>>,
}

fn conv_bench(gpu: &Gpu, name: &str, x: [usize; 4], w: [usize; 4], pads: [i64; 4]) {
    let xt = Tensor::f32(&x, rand_vec(x.iter().product(), 1));
    let wt = Tensor::f32(&w, rand_vec(w.iter().product(), 2));
    let bt = Tensor::f32(&[w[0]], rand_vec(w[0], 3));
    let node = Node { name: "conv".into(), op_type: "Conv".into(), inputs: vec!["x".into(), "w".into(), "b".into()], outputs: vec!["y".into()], attrs: vec![attr_ints("pads", &pads)], ..Default::default() };
    let ins = [Some(&xt), Some(&wt), Some(&bt)];
    let pp = prepack(&node, &ins, 0);
    let ctx = OpCtx { opset: 17, threads: 0, cache: pp.as_ref() };
    let (_, oshape) = nn::conv_geom(&node, &xt.shape, &wt.shape).unwrap();
    let flops = 2.0 * oshape.iter().product::<usize>() as f64 * (w[1] * w[2] * w[3]) as f64;
    let cpu = median_secs(1, 5, || {
        nn::conv(&ctx, &node, &ins).unwrap();
    });
    row(name, "cpu", Some(cpu), Some(flops));
    let m = gpu.be.as_ref().and_then(|be| {
        be.try_run(&node, &ins, &ctx)?.ok()?;
        Some(median_secs(5, 15, || {
            be.try_run(&node, &ins, &ctx).unwrap().unwrap();
        }))
    });
    row(name, "metal", m, Some(flops));
}

fn gemm_bench(n: usize, runs: usize) {
    let a = rand_vec(n * n, 1);
    let b = rand_vec(n * n, 2);
    let flops = 2.0 * (n * n * n) as f64;
    let name = format!("SGEMM {n}x{n}x{n}");
    let mut c = vec![0.0f32; n * n];
    let cpu = median_secs(1, runs, || sgemm(n, n, n, &a, n, 1, &b, n, 1, &mut c, n, false, 0));
    row(&name, "cpu", Some(cpu), Some(flops));
    let m = studio_nn::backend::metal::Metal::shared().and_then(|m| {
        m.sgemm(n, n, n, &a, &b)?;
        Some(median_secs(5, runs * 3, || {
            m.sgemm(n, n, n, &a, &b).unwrap();
        }))
    });
    row(&name, "metal", m, Some(flops));
}

fn cnn_model() -> studio_nn::Model {
    let mut g = GraphBuilder::new(17);
    g.input("x", DType::F32, &[1, 3, 224, 224]);
    let chans = [3usize, 32, 64, 128, 256];
    let mut prev = "x".to_string();
    for i in 0..4 {
        let (ci, co) = (chans[i], chans[i + 1]);
        let (w, b, c, r) = (format!("w{i}"), format!("b{i}"), format!("c{i}"), format!("r{i}"));
        g.init(&w, Tensor::f32(&[co, ci, 3, 3], rand_vec(co * ci * 9, i as u64 + 10).iter().map(|v| v * 0.2).collect()));
        g.init(&b, Tensor::f32(&[co], rand_vec(co, i as u64 + 20)));
        g.node("Conv", &[&prev, &w, &b], &[&c], vec![attr_ints("pads", &[1, 1, 1, 1]), attr_ints("strides", &[2, 2])]);
        g.node("Relu", &[&c], &[&r], vec![]);
        prev = r;
    }
    g.init("pw", Tensor::f32(&[256, 256, 1, 1], rand_vec(256 * 256, 30).iter().map(|v| v * 0.1).collect()));
    g.node("Conv", &[&prev, "pw"], &["pc"], vec![]);
    g.node("GlobalAveragePool", &["pc"], &["gp"], vec![]);
    g.node("Flatten", &["gp"], &["fl"], vec![]);
    g.init("fc", Tensor::f32(&[1000, 256], rand_vec(256_000, 31)));
    g.node("Gemm", &["fl", "fc"], &["logits"], vec![attr_i("transB", 1)]);
    g.output("logits", DType::F32);
    g.build()
}

fn main() {
    let have_metal = metal_available();
    let device = studio_nn::backend::metal::Metal::shared().map(|m| m.device_name().to_string()).unwrap_or_else(|| "none".into());
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    println!("studio-nn benchmarks (release); CPU threads: {threads}; Metal device: {device}");
    println!("median of N runs after warm-up; Metal times include activation upload/readback");
    if let Some(l) = load_average() {
        println!("system load average at start: {l:.1} (other processes skew timings)");
    }
    println!();
    println!("| {:<34} | {:<6} | {:>12} | {:>10} |", "benchmark", "backend", "time", "GFLOP/s");
    println!("|{:-<36}|{:-<8}|{:-<14}|{:-<12}|", "", "", "", "");

    gemm_bench(1024, 7);
    gemm_bench(2048, 3);

    let gpu = Gpu { be: if have_metal { studio_nn::backend::accelerator(BackendKind::Metal, 0.0).ok().flatten() } else { None } };
    conv_bench(&gpu, "Conv 3x3 64->64 @512x512", [1, 64, 512, 512], [64, 64, 3, 3], [1, 1, 1, 1]);
    conv_bench(&gpu, "Conv 1x1 128->128 @256x256", [1, 128, 256, 256], [128, 128, 1, 1], [0, 0, 0, 0]);

    // session-level CNN
    let model = cnn_model();
    let x = Tensor::f32(&[1, 3, 224, 224], rand_vec(3 * 224 * 224, 5));
    let cpu = Session::new(&model, Options::default()).unwrap();
    let t = median_secs(2, 15, || {
        cpu.run(&[("x", x.clone())]).unwrap();
    });
    row("CNN 224x224 (4 conv+1x1+FC) session", "cpu", Some(t), None);
    let m = if have_metal {
        let s = Session::new(&model, Options { backend: BackendKind::Metal, ..Default::default() }).unwrap();
        Some(median_secs(2, 15, || {
            s.run(&[("x", x.clone())]).unwrap();
        }))
    } else {
        None
    };
    row("CNN 224x224 (4 conv+1x1+FC) session", "metal", m, None);

    // audio
    for n in [400usize, 960, 4096] {
        let fft = Fft::new(n);
        let mut re = rand_vec(n, 7);
        let mut im = vec![0.0f32; n];
        let flops = 5.0 * n as f64 * (n as f64).log2();
        let reps = 200;
        let t = median_secs(3, 21, || {
            for _ in 0..reps {
                fft.run(&mut re, &mut im, false);
                fft.run(&mut re, &mut im, true);
                let s = 1.0 / n as f32;
                re.iter_mut().for_each(|v| *v *= s);
                im.iter_mut().for_each(|v| *v *= s);
            }
        }) / (2 * reps) as f64;
        row(&format!("FFT complex n={n}"), "cpu", Some(t), Some(flops));
    }
    let samples: Vec<f32> = rand_vec(16000 * 30, 9).iter().map(|v| v * 0.3).collect();
    let t = median_secs(1, 7, || {
        std::hint::black_box(whisper_log_mel(&samples, 128));
    });
    row("Whisper log-mel 30 s, 128 mels", "cpu", Some(t), None);
}
