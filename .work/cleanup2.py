# -*- coding: utf-8 -*-
"""清理 M2.4 排障探针（第二批）。"""
import io

def clean(p, pairs):
    s = io.open(p, encoding='utf-8').read()
    for old, new in pairs:
        if old not in s:
            print("MISS in", p, ":", old[:50].replace("\n", "\\n"))
            continue
        s = s.replace(old, new)
    io.open(p, 'w', encoding='utf-8', newline='\n').write(s)
    print("cleaned", p)

Q = '"'

clean('crates/aluka-vm/src/interpreter.rs', [
    # Call probe
    ("""                    let callee = self.pop()?;
                    if std::env::var("ALUKA_REQ_DEBUG").is_ok() && num_args >= 1 && self.last_pc < 60 {
                        let d = match &callee {
                            Value::Object(r) => {
                                let kind = match self.heap.get(r.0 as usize) {
                                    Some(HeapObject::Closure { func_idx, .. }) => format!("closure#{func_idx}"),
                                    Some(HeapObject::NativeFn { name, .. }) => format!("native({name})"),
                                    Some(HeapObject::NativeCtor { name, .. }) => format!("ctor({name})"),
                                    other => format!("heap={other:?}"),
                                };
                                format!("obj#{} {kind}", r.0)
                            }
                            v => self.format_value(*v),
                        };
                        eprintln!("[req-dbg] Call@{0} args={num_args} callee={d} stack_len={1} func={2}", self.last_pc, self.stack.len(), self.current_func_idx);
                    }""",
     """                    let callee = self.pop()?;"""),
    # callmethod probe
    ("""                    if std::env::var("ALUKA_REQ_DEBUG").is_ok() && matches!(method_name.as_ref(), "define" | "lookup" | "getOwnPropertyNames") {
                        let r = self.stack.get(self.stack.len().saturating_sub(num_args + 1)).copied().unwrap_or(Value::Undefined);
                        eprintln!("[req-dbg] callmethod {method_name} receiver={r:?} func={} pc={}", self.current_func_idx, self.last_pc);
                    }""",
     ""),
    # getprop probe
    ("""                    if std::env::var("ALUKA_REQ_DEBUG").is_ok() && self.current_func_idx == 8 {
                        eprintln!(
                            "[req-dbg] getprop key={key} obj={obj:?} pc={} locals_ptr={:p} slot0={:?}",
                            self.last_pc,
                            self.locals.as_ptr(),
                            self.locals.first().copied()
                        );
                    }""",
     ""),
    # LoadLocal probe
    ("""                    if std::env::var("ALUKA_REQ_DEBUG").is_ok() && slot == 2 && self.current_func_idx < 0 {
                        let v = self.locals.get(slot).copied().unwrap_or(Value::Undefined);
                        let d = match &v {
                            Value::Object(r) => format!("obj#{}", r.0),
                            other => self.format_value(*other),
                        };
                        eprintln!("[req-dbg] LoadLocal slot2 = {d} locals_len={} last_pc={}", self.locals.len(), self.last_pc);
                    }""",
     ""),
    # try_dispatch hit probe
    ("""                        if std::env::var("ALUKA_REQ_DEBUG").is_ok()
                            && matches!(method_name.as_ref(), "getOwnPropertyNames")
                        {
                            eprintln!("[req-dbg] try_dispatch hit gOPN");
                        }""",
     ""),
])

clean('crates/aluka-vm/src/call.rs', [
    ("""        if std::env::var("ALUKA_REQ_DEBUG").is_ok() && func_idx == 8 {
            let tname = self
                .module_functions
                .get(func_idx)
                .map(|t| format!("{}#{}", t.name, t.source_file))
                .unwrap_or_default();
            eprintln!(
                "[req-dbg] func8 enter tmpl={tname} this={this_val:?} num_locals={} args_n={}",
                tmpl.num_locals,
                args.len()
            );
        }""", ""),
    ("""        if std::env::var("ALUKA_REQ_DEBUG").is_ok() && func_idx == 8 {
            let extras = self
                .module_header_extras
                .get(func_idx)
                .map(|e| format!("args_slot={} no_args={}", e.arguments_slot, e.no_arguments_object))
                .unwrap_or_default();
            eprintln!(
                "[req-dbg] func8 post-bind slot0={:?} locals_len={} extras[{extras}]",
                self.locals.first().copied(),
                self.locals.len()
            );
        }""", ""),
    ("""        if std::env::var("ALUKA_REQ_DEBUG").is_ok() && matches!(tmpl.name.as_str(), "GetIntrinsic" | "getBaseIntrinsic" | "stringToPath" | "callBindBasic") {
            let rv = match &ret {
                Ok(v) => {
                    let d = match v {
                        Value::Object(r) => {
                            let kind = match self.heap.get(r.0 as usize) {
                                Some(HeapObject::Closure { func_idx, .. }) => format!("closure#{func_idx}"),
                                Some(HeapObject::NativeFn { name, .. }) => format!("native({name})"),
                                Some(HeapObject::String(s)) => format!("str({s:?})"),
                                other => format!("heap={other:?}"),
                            };
                            format!("obj#{} {kind}", r.0)
                        }
                        v => self.format_value(*v),
                    };
                    d
                }
                Err(_) => "ERR".to_owned(),
            };
            eprintln!("[req-dbg] invoke-ret {} = {rv}", tmpl.name);
        }""", ""),
    ("""        let desc = self.format_value(callee);
        if std::env::var("ALUKA_REQ_DEBUG").is_ok() {
            let tname = self
                .module_functions
                .get(self.current_func_idx.max(0) as usize)
                .map(|t| format!("{}#{}", t.name, t.source_file))
                .unwrap_or_default();
            let detail = match &callee {
                Value::Object(r) => match self.heap.get(r.0 as usize) {
                    Some(HeapObject::Closure { func_idx, .. }) => format!("closure#{func_idx}"),
                    Some(HeapObject::NativeFn { name, .. }) => format!("native({name})"),
                    Some(HeapObject::NativeCtor { name, .. }) => format!("ctor({name})"),
                    Some(HeapObject::Free) => "FREE-SLOT".to_owned(),
                    other => format!("heap={other:?}"),
                },
                v => format!("prim={v:?}"),
            };
            eprintln!(
                "[req-dbg] not-callable: desc={desc:?} callee={detail} func={} ({tname}) pc={}",
                self.current_func_idx, self.last_pc
            );
        }
        let err = self.alloc_error_instance(&format!("{desc} is not a function"));""",
     """        let desc = self.format_value(callee);
        let err = self.alloc_error_instance(&format!("{desc} is not a function"));"""),
])

clean('crates/aluka-vm/src/property.rs', [
    ("""        if std::env::var("ALUKA_REQ_DEBUG").is_ok() && (key == "extensions" || key == "types") {
            if let Value::Object(r) = obj
                && let Some(HeapObject::Ordinary { props, .. }) = self.heap.get(r.0 as usize)
            {
                let desc = match props {
                    OrdinaryProps::Shape { shape, slots } => {
                        let names = self
                            .shape_table
                            .shape(*shape)
                            .map(|sh| sh.names().collect::<Vec<_>>().join(","))
                            .unwrap_or_default();
                        format!("shape({names}) slots={}", slots.len())
                    }
                    OrdinaryProps::Dict { properties } => format!(
                        "dict({})",
                        properties.keys().cloned().collect::<Vec<_>>().join(",")
                    ),
                };
                let vals = match props {
                    OrdinaryProps::Shape { slots, .. } => slots
                        .iter()
                        .map(|&b| crate::jit_helpers::to_vm_value(b))
                        .map(|v| {
                            match v {
                                Value::Object(rr) => format!("#{}", rr.0),
                                other => self.format_value(other),
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("|"),
                    OrdinaryProps::Dict { properties } => properties
                        .iter()
                        .map(|(k, v)| {
                            format!(
                                "{k}=>{}",
                                match v {
                                    Value::Object(rr) => format!("#{}", rr.0),
                                    other => self.format_value(*other),
                                }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("|"),
                };
                eprintln!(
                    "[req-dbg] get {key} on obj#{} props={desc} vals=[{vals}] func={} pc={}",
                    r.0, self.current_func_idx, self.last_pc
                );
            }
        }""", ""),
    ("""        let value = get_v(self, "value")?;
        if std::env::var("ALUKA_REQ_DEBUG").is_ok() && key == "hello" {
            eprintln!(
                "[req-dbg] ordinary-define key={key} obj={obj:?} value={value:?} has_get={has_get}"
            );
        }
        self.set_property(obj, key, value)""",
     """        let value = get_v(self, "value")?;
        self.set_property(obj, key, value)"""),
])

clean('crates/aluka-vm/src/builtins/global_fns.rs', [
    ("""    if std::env::var("ALUKA_REQ_DEBUG").is_ok() {
        eprintln!(
            "[req-dbg] object_static method={method} pending={} recv={:?} args_n={}",
            super::pending_native_name(),
            current_receiver(),
            args.len()
        );
    }""", ""),
])

clean('crates/aluka-vm/src/builtins/mod.rs', [
    ("""    if std::env::var("ALUKA_REQ_DEBUG").is_ok() {
        eprintln!("[req-dbg] try_dispatch key={key}");
    }""", ""),
])

print("ALL DONE")
