use aporia_bench::corpus::{scan_axis, violates};
use aporia_dsl::lower::compile;

#[test]
fn probe() {
    let src = std::fs::read_to_string(
        "D:/APORIA/software/benchmarks/linear_algebra/quadratic_small_root/model.ap",
    )
    .unwrap();
    let c = compile("q.ap", &src);
    let m = c.model;
    for b in [1.0f64, 2.0, 2.5, 1e2, 700.0, 1e3, 1e4, 1e6, 1e8] {
        print!("b={b:.1} violates={}  ", violates(&m, &[b]));
    }
    println!();
    println!("scan: {:?}", scan_axis(&m, 0, 200, 1e-6));
    let o = aporia_runtime::interp::run(&m, &[1e6], aporia_runtime::ExecConfig::default());
    println!("outs={:?} rule_values={:?}", o.outputs, o.rule_values);
}
