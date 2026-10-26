// Copyright 2026 Ashrox LLC and contributors
// SPDX-License-Identifier: AGPL-3.0-or-later

//! `cargo run --release -p studio-nn --example inspect -- <model> [op]`:
//! prints the model report, and with `op` every node of that type with its
//! inputs and attributes.

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(path) = args.get(1) else {
        eprintln!("usage: inspect <model> [op]");
        return;
    };
    let m = match studio_nn::Model::load(path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("{}", m.report());
    if let Some(op) = args.get(2) {
        for n in m.graph.nodes.iter().filter(|n| n.op_type == *op).take(3) {
            println!("{} {} in={:?} out={:?}", n.domain, n.name, n.inputs, n.outputs);
            for a in &n.attrs {
                println!("   {} = {:?}", a.name, a.value);
            }
        }
    }
}
