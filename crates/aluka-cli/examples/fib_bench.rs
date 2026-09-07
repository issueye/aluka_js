//! VM 执行性能基线（D2 / M0 收口）：fib(30) 字节码解释执行，min-of-5。
//!
//! 负载：`function fib(n){ if(n<2) return n; return fib(n-1)+fib(n-2); }
//! console.log(fib(30));` 经 **Go 前端**编译的 `fib30.bc`（269 万次函数调用、
//! 纯计算压栈）——同时是「Go 前端产物 × Rust VM」跨前端兼容的运行证据。
//!
//! 方法学（总 TODO §1 硬规则）：交替执行 + 轮间冷却 100ms + **min-of-5**。
//! 运行：`cargo run --release -p aluka-cli --example fib_bench`

use aluka_bytecode::BytecodeModule;
use aluka_vm::Vm;
use std::path::Path;
use std::time::{Duration, Instant};

const ROUNDS: u32 = 5;
const COOLDOWN: Duration = Duration::from_millis(100);

fn main() {
    let bc = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fib30.bc");
    let data = std::fs::read(&bc).expect("读取 fib30.bc 失败");
    let module = BytecodeModule::deserialize(&data).expect("反序列化失败");
    module.verify().expect("Verifier 校验失败");

    println!("=== VM 执行基线：fib(30)（标准字节码，min-of-{ROUNDS}，冷却 {COOLDOWN:?}）===");
    let mut best = Duration::MAX;
    for round in 1..=ROUNDS {
        std::thread::sleep(COOLDOWN);
        let mut vm = Vm::new(0);
        vm.set_jit_enabled(false);
        let start = Instant::now();
        vm.run_module(&module).expect("fib30 执行失败");
        let elapsed = start.elapsed();
        best = best.min(elapsed);
        println!("  第 {round} 轮: {elapsed:?}");
    }
    println!("基线（min）: {best:?}");
    let out = &vm_stdout_expect(&module);
    println!("输出校验: {out}");
}

fn vm_stdout_expect(module: &BytecodeModule) -> String {
    let mut vm = Vm::new(0);
    vm.set_jit_enabled(false);
    vm.run_module(module).expect("校验执行失败");
    vm.stdout_records.join("\n")
}
