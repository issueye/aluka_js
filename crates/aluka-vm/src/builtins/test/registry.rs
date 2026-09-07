//! node:test 注册表（Phase 8）：describe/it 收集的 suite 树。
//!
//! 逐函数移植 Node.js 22 LTS 标准（`nodetest/test_registry.go` 的注册数据结构）：
//! 注册表为线程局部单例（JS 执行单线程语义；`cargo test` 并行用例各占
//! 线程互不污染），模块 build 时重置（对齐 Go 每个测试文件运行前
//! `ResetTestRegistry`）。suite 函数体在注册时同步执行（Node 语义），
//! 用例延迟到 `run()` 执行。

use crate::value::Value;
use std::cell::RefCell;
use std::collections::HashMap;

/// 注册的用例节点。
#[derive(Clone)]
pub struct TestNode {
    /// 用例名。
    pub name: String,
    /// 用例函数（skip/todo 允许缺省）。
    pub fn_val: Value,
    /// T1 标记（Node 22 语义）。
    pub skip: bool,
    /// todo 标记（执行但失败不计）。
    pub todo: bool,
    /// only 标记（套件 only 模式下过滤用）。
    pub only: bool,
}

/// 套件内子项（按注册顺序执行——Node 语义）。
#[derive(Clone, Copy)]
pub enum Child {
    /// 子套件（suites 表索引）。
    Suite(usize),
    /// 子用例（tests 表索引）。
    Test(usize),
}

/// 注册的套件节点。
#[derive(Clone)]
pub struct SuiteNode {
    /// 套件名。
    pub name: String,
    /// 父套件（根为 None）。
    pub parent: Option<usize>,
    /// 套件级 before（首用例前执行一次）。
    pub before_hooks: Vec<Value>,
    /// 套件级 after（末用例后执行一次）。
    pub after_hooks: Vec<Value>,
    /// 每个用例前。
    pub before_each: Vec<Value>,
    /// 每个用例后。
    pub after_each: Vec<Value>,
    /// 子项（tests 与 suites 混合，注册顺序）。
    pub children: Vec<Child>,
    /// 子套件索引表（`suiteHasRunnable` 递归用）。
    pub suites: Vec<usize>,
    /// 子用例索引表。
    pub tests: Vec<usize>,
    /// 套件 skip。
    pub skip: bool,
    /// 套件 todo。
    pub todo: bool,
    /// 套件 only。
    pub only: bool,
}

/// suite 树注册表。
#[derive(Clone)]
pub struct Registry {
    /// 套件池（0 号恒为根）。
    pub suites: Vec<SuiteNode>,
    /// 用例池。
    pub tests: Vec<TestNode>,
    /// 当前 describe 栈（栈顶为注册目标）。
    pub stack: Vec<usize>,
}

thread_local! {
    /// 线程局部注册表（模块 build 时重置）。
    static REGISTRY: RefCell<Option<Registry>> = const { RefCell::new(None) };
}

/// 清空注册表（模块 build 时调用，对齐 Go `ResetTestRegistry`）。
pub fn reset() {
    REGISTRY.with(|r| {
        *r.borrow_mut() = Some(Registry {
            suites: vec![SuiteNode {
                name: String::new(),
                parent: None,
                before_hooks: Vec::new(),
                after_hooks: Vec::new(),
                before_each: Vec::new(),
                after_each: Vec::new(),
                children: Vec::new(),
                suites: Vec::new(),
                tests: Vec::new(),
                skip: false,
                todo: false,
                only: false,
            }],
            tests: Vec::new(),
            stack: vec![0],
        });
    });
}

/// 短借执行：注册表存在时以 `&mut Registry` 调用 `f`。
pub fn with<R>(f: impl FnOnce(&mut Registry) -> R) -> Option<R> {
    REGISTRY.with(|r| {
        let mut guard = r.borrow_mut();
        guard.as_mut().map(f)
    })
}

/// 注册用例到当前套件（`it`/`test` 及 skip/todo/only 变体共用）。
pub fn push_test(node: TestNode) {
    with(|reg| {
        let test_idx = reg.tests.len();
        reg.tests.push(node);
        let cur = *reg.stack.last().expect("栈底恒为根");
        reg.suites[cur].children.push(Child::Test(test_idx));
        reg.suites[cur].tests.push(test_idx);
    });
}

/// 注册套件到当前套件并压栈；返回新套件索引。
pub fn push_suite(node: SuiteNode) -> Option<usize> {
    with(|reg| {
        let suite_idx = reg.suites.len();
        let parent = *reg.stack.last().expect("栈底恒为根");
        reg.suites.push(SuiteNode {
            parent: Some(parent),
            ..node
        });
        reg.suites[parent].children.push(Child::Suite(suite_idx));
        reg.suites[parent].suites.push(suite_idx);
        reg.stack.push(suite_idx);
        suite_idx
    })
}

/// describe 函数体执行完毕后弹栈。
pub fn pop_suite() {
    with(|reg| {
        if reg.stack.len() > 1 {
            reg.stack.pop();
        }
    });
}

/// 快照整棵注册表（运行器用：快照后即可在无借用状态下驱动 JS）。
pub fn snapshot() -> Option<Registry> {
    REGISTRY.with(|r| r.borrow().clone())
}

thread_local! {
    /// `register()` 注册的自定义断言（name → fn）。
    static CUSTOM_ASSERTS: RefCell<HashMap<String, Value>> = RefCell::new(HashMap::new());
}

/// 注册自定义断言（挂到后续创建的 `t.assert` 上）。
pub fn register_custom_assert(name: &str, fn_val: Value) {
    CUSTOM_ASSERTS.with(|m| {
        m.borrow_mut().insert(name.to_owned(), fn_val);
    });
}

/// 复制当前自定义断言表（创建 `t.assert` 时并入）。
pub fn take_custom_asserts() -> Vec<(String, Value)> {
    CUSTOM_ASSERTS.with(|m| m.borrow().iter().map(|(k, v)| (k.clone(), *v)).collect())
}

/// 用例选项（Node 语义：skip/todo/only）。
#[derive(Clone, Copy, Default)]
pub struct TestOpts {
    /// 跳过。
    pub skip: bool,
    /// 待办。
    pub todo: bool,
    /// 仅运行。
    pub only: bool,
}

/// 从 options 对象读取 skip/todo/only（对齐 Go `applyTestOpts`）。
pub fn apply_test_opts(vm: &mut crate::interpreter::Vm, o: Value, opts: &mut TestOpts) {
    if let Ok(Value::Boolean(b)) = vm.get_property(o, "skip") {
        opts.skip = b;
    }
    if let Ok(Value::Boolean(b)) = vm.get_property(o, "todo") {
        opts.todo = b;
    }
    if let Ok(Value::Boolean(b)) = vm.get_property(o, "only") {
        opts.only = b;
    }
}

/// 单参形态提取名字与函数（对齐 Go `testNameAndFn`）：
/// 字符串 → 纯名字；函数 → 名字取 `fn.name`；其余 → anonymous。
pub fn test_name_and_fn(vm: &mut crate::interpreter::Vm, args: &[Value]) -> (String, Value) {
    if args.is_empty() {
        return ("anonymous".to_owned(), Value::Undefined);
    }
    if args.len() >= 2 {
        return (vm.format_value(args[0]), args[1]);
    }
    if let Value::Object(r) = args[0] {
        if let Some(crate::heap::HeapObject::String(s)) = vm.heap.get(r.index()) {
            return (s.clone(), Value::Undefined);
        }
    }
    if let Ok(Value::Object(r)) = vm.get_property(args[0], "name") {
        if let Some(crate::heap::HeapObject::String(n)) = vm.heap.get(r.index()) {
            if !n.is_empty() {
                return (n.clone(), args[0]);
            }
        }
    }
    ("anonymous".to_owned(), args[0])
}
