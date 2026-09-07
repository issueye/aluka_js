//! 静态编译期优化 Pass（常量折叠、死代码消除与跳转穿透）。

use crate::scope::CompiledUnit;
use aluka_bytecode::Op;
use aluka_parser::ast::{Expr, Program, PropKey, PropValue, Stmt};

/// 对语法树执行 AST 级静态优化（常量折叠与不可达代码消除）。
pub fn optimize_ast(program: &mut Program) {
    for stmt in &mut program.body {
        optimize_stmt(stmt);
    }
}

/// 递归优化语句
pub fn optimize_stmt(stmt: &mut Stmt) {
    match stmt {
        Stmt::Expr(expr) => {
            optimize_expr(expr);
        }
        Stmt::VarDecl {
            init: Some(init_expr),
            ..
        } => {
            optimize_expr(init_expr);
        }
        Stmt::VarDecl { init: None, .. } => {}
        Stmt::DestructureDecl { init, .. } => {
            optimize_expr(init);
        }
        Stmt::Block(stmts) => {
            for s in stmts {
                optimize_stmt(s);
            }
        }
        Stmt::If {
            cond,
            then_branch,
            else_branch,
        } => {
            optimize_expr(cond);
            optimize_stmt(then_branch);
            if let Some(eb) = else_branch {
                optimize_stmt(eb);
            }
            // 死代码消除：常数已知条件分支消解
            match cond {
                Expr::Boolean(true) => {
                    let taken = std::mem::replace(then_branch.as_mut(), Stmt::Block(Vec::new()));
                    *stmt = taken;
                }
                Expr::Boolean(false) => {
                    if let Some(eb) = else_branch {
                        let taken = std::mem::replace(eb.as_mut(), Stmt::Block(Vec::new()));
                        *stmt = taken;
                    } else {
                        *stmt = Stmt::Block(Vec::new());
                    }
                }
                _ => {}
            }
        }
        Stmt::While { cond, body } => {
            optimize_expr(cond);
            optimize_stmt(body);
            // 死代码消除：while (false) 不发射
            if let Expr::Boolean(false) = cond {
                *stmt = Stmt::Block(Vec::new());
            }
        }
        Stmt::For {
            init,
            cond,
            update,
            body,
        } => {
            if let Some(i) = init {
                optimize_stmt(i);
            }
            if let Some(c) = cond {
                optimize_expr(c);
            }
            if let Some(u) = update {
                optimize_expr(u);
            }
            optimize_stmt(body);
        }
        Stmt::ForIn { right, body, .. } | Stmt::ForOf { right, body, .. } => {
            optimize_expr(right);
            optimize_stmt(body);
        }
        Stmt::Break | Stmt::Continue => {}
        Stmt::Return(Some(expr)) => {
            optimize_expr(expr);
        }
        Stmt::Throw(expr) => {
            optimize_expr(expr);
        }
        Stmt::Function(def) => {
            for s in &mut def.body {
                optimize_stmt(s);
            }
        }
        Stmt::Class {
            constructor,
            methods,
            ..
        } => {
            if let Some(ctor) = constructor {
                for s in &mut ctor.body {
                    optimize_stmt(s);
                }
            }
            for m in methods {
                for s in &mut m.body {
                    optimize_stmt(s);
                }
            }
        }
        _ => {}
    }
}

/// 递归折叠表达式中的纯字面量计算
pub fn optimize_expr(expr: &mut Expr) {
    match expr {
        Expr::Binary { op, left, right } => {
            optimize_expr(left);
            optimize_expr(right);

            // 常量折叠：纯数值二元运算
            if let (Expr::Number(l), Expr::Number(r)) = (left.as_ref(), right.as_ref()) {
                let folded = match op.as_str() {
                    "+" => Some(Expr::Number(l + r)),
                    "-" => Some(Expr::Number(l - r)),
                    "*" => Some(Expr::Number(l * r)),
                    "/" if *r != 0.0 => Some(Expr::Number(l / r)),
                    "%" if *r != 0.0 => Some(Expr::Number(l % r)),
                    "**" => Some(Expr::Number(l.powf(*r))),
                    "<" => Some(Expr::Boolean(l < r)),
                    "<=" => Some(Expr::Boolean(l <= r)),
                    ">" => Some(Expr::Boolean(l > r)),
                    ">=" => Some(Expr::Boolean(l >= r)),
                    "===" | "==" => Some(Expr::Boolean((l - r).abs() < f64::EPSILON)),
                    "!==" | "!=" => Some(Expr::Boolean((l - r).abs() >= f64::EPSILON)),
                    _ => None,
                };
                if let Some(f) = folded {
                    *expr = f;
                }
            }
        }
        Expr::Unary { op, expr: sub } => {
            optimize_expr(sub);
            if let Expr::Number(n) = sub.as_ref() {
                if op == "-" {
                    *expr = Expr::Number(-n);
                } else if op == "+" {
                    *expr = Expr::Number(*n);
                }
            } else if let Expr::Boolean(b) = sub.as_ref() {
                if op == "!" {
                    *expr = Expr::Boolean(!b);
                }
            }
        }
        Expr::Assign { value, .. } => {
            optimize_expr(value);
        }
        Expr::Call { callee, args } | Expr::New { callee, args } => {
            optimize_expr(callee);
            for a in args {
                optimize_expr(a);
            }
        }
        Expr::MethodCall { receiver, args, .. } => {
            optimize_expr(receiver);
            for a in args {
                optimize_expr(a);
            }
        }
        Expr::Array(elements) => {
            for e in elements {
                optimize_expr(e);
            }
        }
        Expr::Spread(inner) => {
            optimize_expr(inner);
        }
        Expr::Object(props) => {
            for p in props {
                if let PropKey::Computed(k) = &mut p.key {
                    optimize_expr(k);
                }
                match &mut p.value {
                    PropValue::Expr(v) | PropValue::Spread(v) => optimize_expr(v),
                    PropValue::Getter(def) | PropValue::Setter(def) => {
                        for s in &mut def.body {
                            optimize_stmt(s);
                        }
                    }
                }
            }
        }
        Expr::Update { target, .. } => {
            optimize_expr(target);
        }
        Expr::Conditional {
            cond,
            then_expr,
            else_expr,
        } => {
            optimize_expr(cond);
            optimize_expr(then_expr);
            optimize_expr(else_expr);
        }
        Expr::OptionalCall { callee, args } => {
            optimize_expr(callee);
            for a in args {
                optimize_expr(a);
            }
        }
        Expr::Function(def) => {
            for stmt in &mut def.body {
                optimize_stmt(stmt);
            }
        }
        Expr::Yield { value: Some(v), .. } => optimize_expr(v),
        Expr::Yield { value: None, .. } => {}
        Expr::Await(arg) => optimize_expr(arg),
        Expr::TemplateLiteral { exprs, .. } => {
            for e in exprs {
                optimize_expr(e);
            }
        }
        Expr::Super => {}
        _ => {}
    }
}

/// 指令级跳转穿透优化（Jump Threading）
pub fn optimize_jumps(unit: &mut CompiledUnit) {
    let len = unit.code.len();
    if len < 2 {
        return;
    }

    for i in 0..len {
        let op = unit.code[i].op;
        if op == Op::Jmp {
            let mut target_pc = unit.code[i].operand;
            let mut hops = 0;
            // 追踪链式无条件跳转
            while (target_pc as usize) < len && hops < 10 {
                let next_op = unit.code[target_pc as usize].op;
                if next_op == Op::Jmp {
                    let next_target = unit.code[target_pc as usize].operand;
                    if next_target == target_pc {
                        break; // 避免死循环
                    }
                    target_pc = next_target;
                    hops += 1;
                } else {
                    break;
                }
            }
            unit.code[i].operand = target_pc;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_constant_folding() {
        // (10 + 20) * 3 => 30 * 3 => 90
        let mut expr = Expr::Binary {
            op: "*".to_owned(),
            left: Box::new(Expr::Binary {
                op: "+".to_owned(),
                left: Box::new(Expr::Number(10.0)),
                right: Box::new(Expr::Number(20.0)),
            }),
            right: Box::new(Expr::Number(3.0)),
        };
        optimize_expr(&mut expr);
        assert_eq!(expr, Expr::Number(90.0));
    }

    #[test]
    fn test_dead_code_elimination() {
        // if (false) { x = 1; } else { x = 2; }
        let mut stmt = Stmt::If {
            cond: Expr::Boolean(false),
            then_branch: Box::new(Stmt::Expr(Expr::Assign {
                name: "x".to_owned(),
                value: Box::new(Expr::Number(1.0)),
            })),
            else_branch: Some(Box::new(Stmt::Expr(Expr::Assign {
                name: "x".to_owned(),
                value: Box::new(Expr::Number(2.0)),
            }))),
        };
        optimize_stmt(&mut stmt);
        assert_eq!(
            stmt,
            Stmt::Expr(Expr::Assign {
                name: "x".to_owned(),
                value: Box::new(Expr::Number(2.0)),
            })
        );
    }
}
