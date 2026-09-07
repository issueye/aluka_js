//! GC 原型基准（T-BE-02 评测数据源）。
//!
//! 负载：`fib30_tree`（递归树形分配）、`churn`（创建-弃置 + 10% 存活）、
//! `cycles`（环形垃圾）。
//!
//! 方法学（总 TODO §1 硬规则）：交替执行 + 轮间冷却 + **min-of-N**
//! （N=5）；数据取最小值以排除环境抖动（Go 侧曾因持续负载 ~5s 降频
//! 险些误判 JIT 回归，前车之鉴）。运行：`cargo run --release -p aluka-core --example gc_bench`

use aluka_core::gc_protos::generational::GenerationalHeap;
use aluka_core::gc_protos::refcount::RefCycleHeap;
use aluka_core::object::ObjectClass;
use aluka_core::value::Value;
use std::time::{Duration, Instant};

/// 每个场景的重复轮数（min-of-N）。
const ROUNDS: u32 = 5;
/// 轮间冷却时长。
const COOLDOWN: Duration = Duration::from_millis(100);

fn main() {
    println!("=== GC 原型基准（min-of-{ROUNDS}，轮间冷却 {COOLDOWN:?}）===");
    println!();

    for &(name, result) in [
        ("fib30_tree/原型A 分代标记-清除", bench_fib_tree_a()),
        ("fib30_tree/原型B 引用计数", bench_fib_tree_b()),
        ("churn/原型A 分代标记-清除", bench_churn_a()),
        ("churn/原型B 引用计数", bench_churn_b()),
        ("cycles/原型A 分代标记-清除", bench_cycles_a()),
        ("cycles/原型B 引用计数", bench_cycles_b()),
    ]
    .iter()
    {
        println!("{name}: {result:?}");
    }
}

/// 场景 1：fib(30) 递归，每层调用分配一个激活记录对象，返回前弃置。
/// 调用次数 ≈ 2·fib(31)-1 ≈ 269 万次——纯分配/回收压力。
fn bench_fib_tree_a() -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        std::thread::sleep(COOLDOWN);
        let mut heap = GenerationalHeap::new();
        let start = Instant::now();
        let mut roots = aluka_core::gc::RootSet::new();
        fib_tree_a(&mut heap, &mut roots, 30);
        let elapsed = start.elapsed();
        let stats = heap.stats();
        best = best.min(elapsed);
        println!(
            "    [A 轮] 耗时 {elapsed:?} 分配 {} minor {} major {} 回收 {}",
            stats.allocated, stats.minor_collections, stats.major_collections, stats.reclaimed
        );
    }
    best
}

fn fib_tree_a(heap: &mut GenerationalHeap, roots: &mut aluka_core::gc::RootSet, n: u32) -> u32 {
    let node = heap.allocate(ObjectClass::Ordinary, 2);
    roots.push(Value::Object(node));
    let result = if n < 2 {
        n
    } else {
        fib_tree_a(heap, roots, n - 1) + fib_tree_a(heap, roots, n - 2)
    };
    // 结果写槽位（真实激活记录会存局部变量；这里仅制造一次写）
    heap.set_slot(node, 0, Value::Number(f64::from(result)));
    roots.pop();
    // 每完成一棵子树做一次 minor（模拟 VM 的分配阈值触发）
    if heap.stats().allocated % 100_000 == 0 {
        heap.collect_minor(roots);
    }
    result
}

fn bench_fib_tree_b() -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        std::thread::sleep(COOLDOWN);
        let mut heap = RefCycleHeap::new();
        let start = Instant::now();
        fib_tree_b(&mut heap, 30);
        let elapsed = start.elapsed();
        let stats = heap.stats();
        best = best.min(elapsed);
        println!(
            "    [B 轮] 耗时 {elapsed:?} 分配 {} 即时释放 {} 循环回收 {}",
            stats.allocated, stats.ref_reclaims, stats.cycle_reclaimed
        );
    }
    best
}

fn fib_tree_b(heap: &mut RefCycleHeap, n: u32) -> u32 {
    let node = heap.allocate(ObjectClass::Ordinary, 2);
    heap.add_root(Value::Object(node));
    let result = if n < 2 {
        n
    } else {
        fib_tree_b(heap, n - 1) + fib_tree_b(heap, n - 2)
    };
    heap.set_slot(node, 0, Value::Number(f64::from(result)));
    heap.remove_root(Value::Object(node)); // 弃置：RC 即时释放
    result
}

/// 场景 2：创建-弃置循环。20 万次分配，10% 存入持久列表（跨 GC 存活）。
fn bench_churn_a() -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        std::thread::sleep(COOLDOWN);
        let mut heap = GenerationalHeap::new();
        let mut persistent: Vec<Value> = Vec::new();
        let mut roots = aluka_core::gc::RootSet::new();
        let start = Instant::now();
        for i in 0..200_000u32 {
            let obj = heap.allocate(ObjectClass::Ordinary, 2);
            heap.set_slot(obj, 0, Value::Number(f64::from(i)));
            if i % 10 == 0 {
                persistent.push(Value::Object(obj));
            }
            if i % 100_000 == 99_999 {
                roots.clear();
                for v in &persistent {
                    roots.push(*v);
                }
                heap.collect_minor(&roots);
            }
        }
        let elapsed = start.elapsed();
        let stats = heap.stats();
        best = best.min(elapsed);
        println!(
            "    [A 轮] 耗时 {elapsed:?} 存活 {} minor {} 回收 {}",
            stats.live, stats.minor_collections, stats.reclaimed
        );
    }
    best
}

fn bench_churn_b() -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        std::thread::sleep(COOLDOWN);
        let mut heap = RefCycleHeap::new();
        let start = Instant::now();
        for i in 0..200_000u32 {
            let obj = heap.allocate(ObjectClass::Ordinary, 2);
            heap.add_root(Value::Object(obj)); // RC 完整生命周期：分配即登记
            heap.set_slot(obj, 0, Value::Number(f64::from(i)));
            if i % 10 == 0 {
                // 存活：根保留（模拟持久容器持有）
            } else {
                heap.remove_root(Value::Object(obj)); // 弃置：即时释放
            }
            if i % 100_000 == 99_999 {
                heap.collect_cycles();
            }
        }
        let elapsed = start.elapsed();
        let stats = heap.stats();
        best = best.min(elapsed);
        println!(
            "    [B 轮] 耗时 {elapsed:?} 存活 {} 即时释放 {} 循环回收 {}",
            stats.live, stats.ref_reclaims, stats.cycle_reclaimed
        );
    }
    best
}

/// 场景 3：5 万个三节点环，90% 弃置、10% 挂根。
fn bench_cycles_a() -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        std::thread::sleep(COOLDOWN);
        let mut heap = GenerationalHeap::new();
        let mut roots = aluka_core::gc::RootSet::new();
        let mut kept = 0u32;
        let start = Instant::now();
        for i in 0..50_000u32 {
            let a = heap.allocate(ObjectClass::Ordinary, 1);
            let b = heap.allocate(ObjectClass::Ordinary, 1);
            let c = heap.allocate(ObjectClass::Ordinary, 1);
            heap.set_slot(a, 0, Value::Object(b));
            heap.set_slot(b, 0, Value::Object(c));
            heap.set_slot(c, 0, Value::Object(a));
            if i % 10 == 0 {
                roots.push(Value::Object(a));
                kept += 1;
            }
            if i % 100_000 == 99_999 {
                heap.collect_minor(&roots);
            }
        }
        heap.collect_major(&roots);
        let elapsed = start.elapsed();
        let stats = heap.stats();
        best = best.min(elapsed);
        println!(
            "    [A 轮] 耗时 {elapsed:?} 挂根 {kept} 存活 {} 回收 {}",
            stats.live, stats.reclaimed
        );
    }
    best
}

fn bench_cycles_b() -> Duration {
    let mut best = Duration::MAX;
    for _ in 0..ROUNDS {
        std::thread::sleep(COOLDOWN);
        let mut heap = RefCycleHeap::new();
        let start = Instant::now();
        for i in 0..50_000u32 {
            let a = heap.allocate(ObjectClass::Ordinary, 1);
            let b = heap.allocate(ObjectClass::Ordinary, 1);
            let c = heap.allocate(ObjectClass::Ordinary, 1);
            heap.add_root(Value::Object(a));
            heap.add_root(Value::Object(b));
            heap.add_root(Value::Object(c));
            heap.set_slot(a, 0, Value::Object(b));
            heap.set_slot(b, 0, Value::Object(c));
            heap.set_slot(c, 0, Value::Object(a));
            if i % 10 != 0 {
                // 弃置：断开全部外部根，环整体成为垃圾（RC 需循环回收兜底）
                heap.remove_root(Value::Object(a));
                heap.remove_root(Value::Object(b));
                heap.remove_root(Value::Object(c));
            }
            if i % 100_000 == 99_999 {
                heap.collect_cycles();
            }
        }
        heap.collect_cycles();
        let elapsed = start.elapsed();
        let stats = heap.stats();
        best = best.min(elapsed);
        println!(
            "    [B 轮] 耗时 {elapsed:?} 存活 {} 循环回收 {}",
            stats.live, stats.cycle_reclaimed
        );
    }
    best
}
