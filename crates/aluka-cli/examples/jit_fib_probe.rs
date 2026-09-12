//! 诊断探针：fib30.bc 以 JIT 开启执行，测量直调覆盖率与耗时（切片四 P1）。
use aluka_bytecode::BytecodeModule;
use aluka_vm::Vm;
use aluka_vm::heap::HeapObject;
use std::path::Path;
use std::time::Instant;

fn main() {
    let bc = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/fib30.bc");
    let data = std::fs::read(&bc).expect("读取 fib30.bc 失败");
    let module = BytecodeModule::deserialize(&data).expect("反序列化失败");
    module.verify().expect("Verifier 校验失败");
    for round in 1..=3 {
        let mut vm = Vm::new(0);
        let start = Instant::now();
        let out = vm.run_module(&module).expect("fib30 执行失败");
        let el = start.elapsed();
        println!(
            "round {round}: {el:?} out={out:?} fallbacks={} compiled={}",
            vm.jit_call_fallbacks(),
            vm.jit_compiled_count()
        );
        if round == 1 {
            if let Some(fibv) = vm.globals.get("fib") {
                if let Some(r) = fibv.as_object() {
                    if let Some(HeapObject::Closure {
                        func_idx, upvalues, ..
                    }) = vm.heap.get(r.index())
                    {
                        println!(
                            "fib: func_idx={func_idx} upvalues={} slot={}",
                            upvalues.len(),
                            vm.jit_slot_summary(*func_idx)
                        );
                    } else {
                        println!("fib: 非 Closure 堆对象");
                    }
                } else {
                    println!("fib: 非对象");
                }
            } else {
                println!(
                    "fib: 不在 globals（键样例：{:?}）",
                    vm.globals.keys().take(6).collect::<Vec<_>>()
                );
            }
        }
    }
}
