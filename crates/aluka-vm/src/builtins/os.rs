//! `os` 内置模块扩展（Phase 2）：`arch` / `release` / `type` / `cpus` / `userInfo`。
//!
//! 单例说明：解释器在 `register_all` **之后**才创建 `os_module`（interpreter.rs
//! `Vm::new` 顺序），因此 build 阶段取不到同一单例。这里在 build 时创建模块
//! 对象并回填 `vm.os_module`，同时所有处理器入口做一次惰性重链
//! （[`sync_os_link`]）：首个注册表方法被调用时，把运行时单例重新挂进
//! `registry.modules["os"]`，保证 `CALL_METHOD` 形态二（模块单例直调）命中。
//! 探测脚本在调用 `os.*` 之前先调用任一注册表方法（如 `util.format("")`）
//! 即可完成重链；重链后两对象相同，其余模块不受影响。
//!
//! 语义对齐 Node.js 22 LTS 标准（`nodeos`）：`arch` 按 GOARCH 映射（amd64→x64）、
//! `type` 为 `Windows_NT`/`Linux`、`release` 为 Windows `10.0.xxxxx` 三段式、
//! `cpus` 返回 `[{model:"unknown", speed:0, times:{user:0,nice:0,sys:0,idle:0,irq:0}}]`
//! 简化单 CPU、`userInfo().username` 取环境变量 `USER` 优先 `USERNAME`。

use crate::builtins::{BuiltinRegistry, ModuleDef, register_handler, set_module_prop};
use crate::interpreter::{Vm, VmError};
use crate::value::Value;
use aluka_core::ObjectRef;

/// `require("os")` / `require("node:os")` 主模块（扩展 arch/release/type/cpus/userInfo）。
pub const MODULE: ModuleDef = ModuleDef { name: "os", build };

fn build(vm: &mut Vm, registry: &mut BuiltinRegistry) -> Result<ObjectRef, VmError> {
    let obj = match vm.os_module {
        Some(r) => r,
        None => {
            let r = vm.alloc_ordinary();
            vm.os_module = Some(r);
            r
        }
    };
    for method in ["arch", "release", "type", "cpus", "userInfo"] {
        let fn_ref = vm.alloc_native_fn(&format!("os.{method}"));
        set_module_prop(vm, obj, method, Value::Object(fn_ref))?;
    }
    register_handler(registry, "os", "arch", arch);
    register_handler(registry, "os", "release", release);
    register_handler(registry, "os", "type", type_name);
    register_handler(registry, "os", "cpus", cpus);
    register_handler(registry, "os", "userInfo", user_info);
    Ok(obj)
}

/// 惰性重链：运行时 `os` 单例是 `Vm::new` 在 `register_all` 之后创建的，把
/// 它挂回注册表，使 `os.arch()` 等经 `CALL_METHOD` 形态二分派命中处理器。
/// 两对象相同时为 no-op（解释器时序修正后自动退化为空操作）。
fn sync_os_link(vm: &mut Vm) {
    if let Some(cur) = vm.os_module {
        if vm.builtin_registry.module("os") != Some(cur) {
            vm.builtin_registry.modules.insert("os", cur);
        }
    }
}

/// `os.arch()`：GOARCH 映射（对齐 Go `archName` 的 Node 风格命名）。
fn arch(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let name = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "x86" => "ia32",
        "aarch64" => "arm64",
        other => other,
    };
    Ok(Value::Object(vm.alloc_string(name.to_owned())))
}

/// `os.type()`：`Windows_NT` / `Linux` / `Darwin`（对齐 Go `osTypeName`）。
fn type_name(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let name = if cfg!(windows) {
        "Windows_NT"
    } else if cfg!(target_os = "macos") {
        "Darwin"
    } else {
        "Linux"
    };
    Ok(Value::Object(vm.alloc_string(name.to_owned())))
}

/// `os.release()`：Windows 取 `cmd /c ver` 前三段（如 10.0.26200，与 Go
/// RtlGetVersion 的 major.minor.build 口径一致）；其余平台读取
/// `/proc/sys/kernel/osrelease` 前三段。
fn release(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let version = if cfg!(windows) {
        windows_release()
    } else {
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|s| first_three_dotted(&s))
            .unwrap_or_default()
    };
    Ok(Value::Object(vm.alloc_string(version)))
}

#[cfg(windows)]
fn windows_release() -> String {
    let out = std::process::Command::new("cmd")
        .args(["/c", "ver"])
        .output()
        .ok();
    let text = out
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    first_three_dotted(&text)
}

#[cfg(not(windows))]
fn windows_release() -> String {
    String::new()
}

/// 取文本中首个「数字(.数字)+」片段的前三段（如 `10.0.26200.9168` → `10.0.26200`）。
fn first_three_dotted(text: &str) -> String {
    let digits: String = text
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.split('.').take(3).collect::<Vec<_>>().join(".")
}

/// `os.cpus()`：简化实现——单 CPU 信息对象数组（对齐 Go `osCPUs` 的字段形态）。
fn cpus(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let cpu = vm.alloc_ordinary();
    let model = vm.alloc_string("unknown".to_owned());
    let _ = vm.set_property(Value::Object(cpu), "model", Value::Object(model));
    let _ = vm.set_property(Value::Object(cpu), "speed", Value::Number(0.0));
    let times = vm.alloc_ordinary();
    for k in ["user", "nice", "sys", "idle", "irq"] {
        let _ = vm.set_property(Value::Object(times), k, Value::Number(0.0));
    }
    let _ = vm.set_property(Value::Object(cpu), "times", Value::Object(times));
    Ok(Value::Object(vm.alloc_array(vec![Value::Object(cpu)])))
}

/// `os.userInfo()`：`{ homedir, username, shell, uid, gid }`（Windows 形态）。
fn user_info(vm: &mut Vm, _args: &[Value]) -> Result<Value, VmError> {
    sync_os_link(vm);
    let info = vm.alloc_ordinary();
    let homedir = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let homedir_s = vm.alloc_string(homedir);
    let username_s = vm.alloc_string(username);
    let _ = vm.set_property(Value::Object(info), "homedir", Value::Object(homedir_s));
    let _ = vm.set_property(Value::Object(info), "username", Value::Object(username_s));
    // Go Windows 实测 shell 为 null、uid/gid 为 -1。
    let _ = vm.set_property(Value::Object(info), "shell", Value::Null);
    let _ = vm.set_property(Value::Object(info), "uid", Value::Number(-1.0));
    let _ = vm.set_property(Value::Object(info), "gid", Value::Number(-1.0));
    Ok(Value::Object(info))
}

/// 编译期锚定：处理器签名与注册表一致。
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handler_signatures_anchor() {
        let _: crate::builtins::BuiltinHandler = arch;
        let _: crate::builtins::BuiltinHandler = release;
        let _: crate::builtins::BuiltinHandler = type_name;
        let _: crate::builtins::BuiltinHandler = cpus;
        let _: crate::builtins::BuiltinHandler = user_info;
    }

    #[test]
    fn release_parses_first_three_parts() {
        assert_eq!(
            first_three_dotted("Microsoft Windows [版本 10.0.26200.9168]"),
            "10.0.26200"
        );
        assert_eq!(first_three_dotted("x 1.2.3.4 y"), "1.2.3");
        assert_eq!(first_three_dotted("no digits"), "");
    }
}
