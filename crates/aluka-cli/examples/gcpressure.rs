//! gcPressure 内存基准（M6.1 验收：对标 Node.js 22 (V8) 的峰值内存 2~3x 以内）。
//!
//! 工作负载：400 波 × 5 万个三字段对象（每波丢弃，制造持续回收压力，
//! 共 2000 万对象；V8 逃逸优化可能削减其有效分配，故比值偏保守）。同一脚本分别在 node 与 aluvm（字节码）上运行；单个
//! PowerShell 进程以 60ms 轮询子进程工作集直至其退出，取峰值对比。
//!
//! 运行（非测试目标，不进门禁）：
//! ```powershell
//! cargo run -p aluka-cli --features runtime --example gcpressure
//! ```

use std::io::Read as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

const WORKLOAD: &str = r#"
// gcPressure：波浪式分配（每波构建后丢弃，制造持续 GC 压力）
let sink = 0;
for (let w = 0; w < 400; w++) {
  const keep = [];
  for (let i = 0; i < 50000; i++) {
    keep.push({ id: i, name: 'item' + i, tags: [i, i + 1, i + 2] });
  }
  sink += keep.length;
}
console.log('total ' + sink);
"#;

/// 运行子进程并用单个 PowerShell 轮询器采样峰值工作集（KB）。
/// 返回 (峰值 KB, 耗时 ms)。
fn run_and_measure(exe: &str, arg: &str, cwd: &std::path::Path) -> (f64, u128) {
    let mut child = Command::new(exe)
        .arg(arg)
        .current_dir(cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("子进程启动失败");
    let pid = child.id();
    let started = Instant::now();
    // 单 PS 进程内循环轮询（规避 PS 冷启动 ~1s 错过短进程采样）
    let ps_script = format!(
        "$peak=0; while($true) {{ $p = Get-Process -Id {pid} -ErrorAction SilentlyContinue; if(-not $p) {{ break }}; if($p.WorkingSet64 -gt $peak) {{ $peak = $p.WorkingSet64 }}; Start-Sleep -Milliseconds 60 }}; Write-Output ([math]::Round($peak / 1KB, 1))"
    );
    let mut sampler = Command::new("powershell")
        .args(["-NoProfile", "-Command", &ps_script])
        .stdout(Stdio::piped())
        .spawn()
        .expect("采样器启动");
    let _ = child.wait();
    let elapsed = started.elapsed().as_millis();
    // 采样器随被测进程退出自然收尾：读全量输出取峰值，并回收采样进程
    let mut peak_kb = 0.0f64;
    let mut buf = String::new();
    if let Some(mut out) = sampler.stdout.take() {
        let _ = out.read_to_string(&mut buf);
    }
    let _ = sampler.wait();
    if let Ok(kb) = buf.trim().parse::<f64>() {
        peak_kb = kb;
    }
    (peak_kb, elapsed)
}

fn main() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bins = manifest
        .join("../../target/debug")
        .canonicalize()
        .expect("target/debug 存在（先 cargo build -p aluka-cli --features runtime）");
    let alukac = bins.join("alukac.exe");
    let aluvm = bins.join("aluvm.exe");

    let tmp = std::env::temp_dir().join(format!("aluka_gcpressure_{}", std::process::id()));
    std::fs::create_dir_all(&tmp).expect("创建基准临时目录");
    let workload = tmp.join("workload.js");
    std::fs::write(&workload, WORKLOAD).expect("写入工作负载");

    // 1. 编译字节码
    let bc = tmp.join("workload.bc");
    let compile = Command::new(&alukac)
        .args(["compile", &workload.display().to_string(), "-o"])
        .arg(&bc)
        .output()
        .expect("alukac 可执行");
    assert!(compile.status.success(), "编译失败");

    // 2. node 基线（峰值工作集采样）
    let node = std::env::var("NODE").unwrap_or_else(|_| "node".to_owned());
    let wl = workload.display().to_string();
    let (node_kb, node_ms) = run_and_measure(&node, &wl, &tmp);

    // 3. aluvm（字节码直载）
    let bc_arg = bc.display().to_string();
    let (aluka_kb, aluka_ms) = run_and_measure(aluvm.to_str().unwrap(), &bc_arg, &tmp);

    let _ = std::fs::remove_dir_all(&tmp);

    let ratio = aluka_kb / node_kb.max(1.0);
    println!("gcPressure 基准（400 波 × 5 万对象 = 2000 万；debug 构建）");
    println!(
        "  node  峰值工作集: {:>8.1} MB  耗时 {:>6} ms",
        node_kb / 1024.0,
        node_ms
    );
    println!(
        "  aluka 峰值工作集: {:>8.1} MB  耗时 {:>6} ms",
        aluka_kb / 1024.0,
        aluka_ms
    );
    println!("  内存比 aluka/node: {ratio:.2}x（M6.1 验收线 ≤ 3.0x）");
    if ratio <= 3.0 && node_kb > 0.0 {
        println!("  判定: PASS");
    } else {
        println!("  判定: FAIL（超出 3x 验收线或采样失败）");
        std::process::exit(1);
    }
}
