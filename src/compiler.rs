use crate::structs::*;
use Arg64::*;
use Instr::*;
use MovArgs::*;
use Reg::*;

use dynasmrt::DynamicLabel;

use im::hashmap;
use im::HashMap;

const TRUE: Arg64 = Imm(3);
const FALSE: Arg64 = Imm(1);

pub fn depth(e: &Expr) -> i32 {
    match e {
        Expr::Num(_) => 0,
        Expr::Var(_) => 0,
        Expr::Boolean(_) => 0,
        Expr::UnOp(_, e) => depth(e),
        // Right to left evaluation order
        Expr::BinOp(_, e1, e2) => depth(e2).max(depth(e1) + 1),
        Expr::Let(bindings, e) => bindings
            .iter()
            .enumerate()
            .map(|(i, (_, e))| (i as i32 + depth(e)))
            .max()
            .unwrap_or(0)
            .max(bindings.len() as i32 + depth(e)),
        Expr::If(cond, then, other) => depth(cond).max(depth(then)).max(depth(other)),
        Expr::Loop(e) => depth(e),
        Expr::Block(es) => es.iter().map(|expr| depth(expr)).max().unwrap_or(0),
        Expr::Break(e) => depth(e),
        Expr::Set(_, e) => depth(e),
        Expr::Define(_, e) => depth(e),
        Expr::FnDefn(_, v, b) => depth_aligned(b, v.len() as i32),
        Expr::FnCall(_, args) => args.len() as i32,
    }
}

fn depth_aligned(e: &Expr, extra: i32) -> i32 {
    // Off aligned 16byte depth
    let d = depth(e) + extra;
    if d % 2 != 0 {
        d
    } else {
        d + 1
    }
}

pub fn compile_func_defns(fns: &Vec<Expr>, com: &mut ContextMut) -> Vec<Instr> {
    let mut instrs: Vec<Instr> = vec![];

    // Preprocess all function definitions
    com.fns.extend(fns.iter().fold(hashmap! {}, |mut acc, f| {
        if let Expr::FnDefn(n, v, b) = f {
            if acc.get(n).is_some() {
                panic!("function redefined")
            }
            acc.insert(
                n.to_string(),
                FunEnv::new(v.len() as i32, depth_aligned(b, v.len() as i32)),
            );
            return acc;
        }
        // Should not happen, since we are catching it in parse
        panic!("Invalid: cannot compile anything other than function definitions here")
    }));

    for f in fns {
        // No else block as we checked and paniced in preprocessing
        if let Expr::FnDefn(name, vars, body) = f {
            com.depth = com.fns.get_mut(name).unwrap().depth;
            // Separate context for each function definiton
            let mut co = Context::new(None)
                .modify_si(vars.len() as i32)
                // Function body is tail position
                .modify_tail(true);

            for (i, v) in vars.iter().enumerate() {
                let existing = co.env.get(v.as_str());
                if existing.is_some() && !existing.unwrap().in_heap {
                    panic!("duplicate parameter binding in definition");
                }
                co.env
                    .insert(v.to_string(), VarEnv::new(i as i32, None, false));
            }

            instrs.push(LabelI(Label::new(Some(&format!("fun_{name}")))));
            instrs.push(Sub(ToReg(Rsp, Imm(com.depth * 8))));
            instrs.extend(compile_expr(body, &co, com));
            instrs.push(Add(ToReg(Rsp, Imm(com.depth * 8))));
            instrs.push(Ret);
        }
    }
    return instrs;
}

pub fn compile_expr_aligned(
    e: &Expr,
    co: Option<&Context>,
    com: Option<&mut ContextMut>,
    input: Option<bool>,
) -> Vec<Instr> {
    // Top level is not a tail position
    let mut co_ = &Context::new(None).modify_si(1);
    if let Some(x) = co {
        co_ = x;
    };
    let co = co_;
    let mut com_ = &mut ContextMut::new();
    if let Some(x) = com {
        com_ = x;
    }
    let com = com_;

    com.depth = depth_aligned(e, if let Some(_) = input { 3 } else { 2 }); // 1 extra for input

    let mut instrs: Vec<Instr> = vec![
        Sub(ToReg(Rsp, Imm(com.depth * 8))),
        Mov(ToMem(
            MemRef {
                reg: Rsp,
                offset: 0,
            },
            OReg(Rdi),
        )),
    ];
    instrs.extend(compile_expr(
        e,
        &co.modify_env(
            co.env
                .update("input".to_string(), VarEnv::new(0, input, false)),
        ),
        com,
    ));
    instrs.push(Add(ToReg(Rsp, Imm(com.depth * 8))));
    return instrs;
}

pub fn compile_expr(e: &Expr, co: &Context, com: &mut ContextMut) -> Vec<Instr> {
    let mut instrs: Vec<Instr> = vec![];
    let snek_error: Label = Label::new(Some("snek_error_stub"));

    match e {
        Expr::Num(n) => {
            let (i, overflow) = n.overflowing_mul(2);
            if overflow {
                panic!("Invalid");
            }

            if let Ok(n) = i32::try_from(i) {
                instrs.push(Mov(co.src_to_target(Imm(n))));
            } else {
                instrs.push(Mov(ToReg(Rax, Imm64(i))));
                co.rax_to_target(&mut instrs);
            }

            com.result_is_bool = Some(false);
        }
        Expr::Boolean(b) => {
            let res = match b {
                true => TRUE,
                false => FALSE,
            };
            instrs.push(Mov(co.src_to_target(res)));
            com.result_is_bool = Some(true);
        }
        Expr::Var(x) => {
            // Get variable's VarEnv
            let venv = match if co.env.contains_key(x) {
                &co.env
            } else {
                &com.env
            }
            .get(x)
            {
                Some(o) => o,
                None => panic!("Unbound variable identifier {x}"),
            };

            let reg = if venv.in_heap {
                instrs.push(Mov(ToReg(Rbx, Imm64(co.get_heap()))));
                Rbx
            } else {
                Rsp
            };
            instrs.push(Mov(ToReg(
                Rax,
                Mem(MemRef {
                    reg,
                    offset: venv.offset,
                }),
            )));
            co.rax_to_target(&mut instrs);
            com.result_is_bool = venv.is_bool;
        }
        Expr::UnOp(op, subexpr) => {
            // UnOp operand cannot be a tail position
            let co = &co.modify_tail(false);
            instrs.extend(compile_expr(subexpr, co, com));

            match op {
                Op1::Add1 => {
                    // Check if Rax is number
                    if com.result_is_bool.is_none() {
                        instrs.push(Test(co.src_to_target(Imm(1))));
                        instrs.push(Mov(ToReg(Rdi, Imm(20)))); // invalid argument
                        instrs.push(JumpI(Jump::Nz(snek_error.clone())));
                    } else if com.result_is_bool.unwrap() {
                        instrs.push(Mov(ToReg(Rdi, Imm(20)))); // invalid argument
                        instrs.push(JumpI(Jump::U(snek_error.clone())));
                        com.result_is_bool = Some(false);
                        return instrs;
                    }
                    instrs.push(Add(co.src_to_target(Imm(2))));
                    // Check overflow
                    instrs.push(Mov(ToReg(Rdi, Imm(30))));
                    instrs.push(JumpI(Jump::O(snek_error)));
                    com.result_is_bool = Some(false);
                }
                Op1::Sub1 => {
                    // Check if Rax is number
                    if com.result_is_bool.is_none() {
                        instrs.push(Test(co.src_to_target(Imm(1))));
                        instrs.push(Mov(ToReg(Rdi, Imm(20)))); // invalid argument
                        instrs.push(JumpI(Jump::Nz(snek_error.clone())));
                    } else if com.result_is_bool.unwrap() {
                        instrs.push(Mov(ToReg(Rdi, Imm(20)))); // invalid argument
                        instrs.push(JumpI(Jump::U(snek_error.clone())));
                        com.result_is_bool = Some(false);
                        return instrs;
                    }
                    instrs.push(Sub(co.src_to_target(Imm(2))));
                    // Check overflow
                    instrs.push(Mov(ToReg(Rdi, Imm(30))));
                    instrs.push(JumpI(Jump::O(snek_error)));
                    com.result_is_bool = Some(false);
                }
                Op1::IsBool => {
                    instrs.push(And(co.src_to_target(Imm(1))));
                    instrs.push(Mov(ToReg(Rax, TRUE))); // Set true
                    instrs.push(Mov(ToReg(Rbx, FALSE)));
                    instrs.push(CMovI(CMov::Z(Rax, OReg(Rbx)))); // Set false if zero
                    co.rax_to_target(&mut instrs);

                    com.result_is_bool = Some(true);
                }
                Op1::IsNum => {
                    instrs.push(And(co.src_to_target(Imm(1))));
                    instrs.push(Mov(ToReg(Rax, FALSE))); // Set false
                    instrs.push(Mov(ToReg(Rbx, TRUE)));
                    instrs.push(CMovI(CMov::Z(Rax, OReg(Rbx)))); // Set true if zero
                    co.rax_to_target(&mut instrs);

                    com.result_is_bool = Some(true);
                }
                Op1::Print => {
                    instrs.extend(co.target_to_reg(Rdi));
                    instrs.push(Call(Label::new(Some("snek_print"))));
                }
            }
        }
        Expr::BinOp(op, left, right) => {
            // BinOp operands cannot be a tail position
            let co = &co.modify_tail(false);
            instrs.extend(compile_expr(
                right,
                &co.modify_target(Some(MemRef {
                    reg: Rsp,
                    offset: co.si,
                })),
                com,
            ));
            let rtype = com.result_is_bool;

            instrs.extend(compile_expr(
                left,
                &co.modify(Some(co.si + 1), None, None, Some(None), None),
                com,
            ));
            let ltype = com.result_is_bool;

            let mem = Mem(MemRef {
                reg: Rsp,
                offset: co.si,
            });

            if let Op2::Equal = op {
                let needs_check = ltype.is_none() || rtype.is_none();
                if ltype.is_some() && rtype.is_some() && ltype != rtype {
                    instrs.push(Mov(ToReg(Rdi, Imm(21)))); // invalid argument
                    instrs.push(JumpI(Jump::Nz(snek_error.clone())));
                    com.result_is_bool = Some(true);
                    return instrs;
                }
                // Check equality with sub instead of cmp
                instrs.push(Sub(ToReg(Rax, mem)));
                if needs_check {
                    instrs.push(Push(Rax)); // Push to stack for checking type later
                }
                instrs.push(Mov(ToReg(Rax, FALSE))); // Set false
                instrs.push(Mov(ToReg(Rbx, TRUE)));
                instrs.push(CMovI(CMov::E(Rax, OReg(Rbx))));

                if needs_check {
                    // Check if both were of the same type
                    instrs.push(Pop(Rbx));
                    instrs.push(Test(ToReg(Rbx, Imm(1))));
                    instrs.push(Mov(ToReg(Rdi, Imm(22)))); // invalid argument
                    instrs.push(JumpI(Jump::Nz(snek_error.clone())));
                }
                com.result_is_bool = Some(true);
            } else {
                // Check if Rax and mem is a number
                if (ltype.is_some() && ltype.unwrap()) || (rtype.is_some() && rtype.unwrap()) {
                    instrs.push(Mov(ToReg(Rdi, Imm(23)))); // invalid argument
                    instrs.push(JumpI(Jump::U(snek_error.clone())));
                    com.result_is_bool = Some(if let Op2::Plus | Op2::Minus | Op2::Times = op {
                        false
                    } else {
                        true
                    });
                    return instrs;
                }
                if ltype.is_none() {
                    instrs.push(Test(ToReg(Rax, Imm(1))));
                    instrs.push(Mov(ToReg(Rdi, Imm(24)))); // invalid argument
                    instrs.push(JumpI(Jump::Nz(snek_error.clone())));
                }
                if rtype.is_none() {
                    instrs.push(Test(ToMem(
                        MemRef {
                            reg: Rsp,
                            offset: co.si,
                        },
                        Imm(1),
                    )));
                    instrs.push(Mov(ToReg(Rdi, Imm(25)))); // invalid argument
                    instrs.push(JumpI(Jump::Nz(snek_error.clone())));
                }

                if let Op2::Plus | Op2::Minus | Op2::Times = op {
                    match op {
                        Op2::Plus => instrs.push(Add(ToReg(Rax, mem))),
                        Op2::Minus => instrs.push(Sub(ToReg(Rax, mem))),
                        Op2::Times => {
                            instrs.push(Sar(Rax, 1));
                            instrs.push(Mul(Rax, mem));
                        }
                        _ => panic!("should not happen"),
                    }
                    instrs.push(Mov(ToReg(Rdi, Imm(32)))); // overflow
                    instrs.push(JumpI(Jump::O(snek_error)));
                    com.result_is_bool = Some(false);
                } else {
                    instrs.push(Cmp(ToReg(Rax, mem)));
                    instrs.push(Mov(ToReg(Rax, FALSE))); // Set false
                    instrs.push(Mov(ToReg(Rbx, TRUE)));
                    match op {
                        Op2::Greater => instrs.push(CMovI(CMov::G(Rax, OReg(Rbx)))),
                        Op2::GreaterEqual => instrs.push(CMovI(CMov::GE(Rax, OReg(Rbx)))),
                        Op2::Less => instrs.push(CMovI(CMov::L(Rax, OReg(Rbx)))),
                        Op2::LessEqual => instrs.push(CMovI(CMov::LE(Rax, OReg(Rbx)))),
                        _ => panic!("should not happen"),
                    }
                    com.result_is_bool = Some(true);
                }
            };
            co.rax_to_target(&mut instrs);
        }
        Expr::Let(bindings, e) => {
            let mut new_env = co.env.clone();
            let mut track_dup = hashmap! {};

            for (i, (x, b)) in bindings.iter().enumerate() {
                let si_ = co.si + i as i32;
                if track_dup.contains_key(x) {
                    panic!("Duplicate binding")
                }
                instrs.extend(compile_expr(
                    b,
                    &co.modify(
                        Some(si_),
                        Some(new_env.clone()),
                        None,
                        Some(Some(MemRef {
                            reg: Rsp,
                            offset: si_,
                        })),
                        // Let binding is not a tail position
                        Some(false),
                    ),
                    com,
                ));
                track_dup.insert(x.to_string(), true);
                new_env.insert(x.to_string(), VarEnv::new(si_, com.result_is_bool, false));
            }

            instrs.extend(compile_expr(
                e,
                &co.modify(
                    Some(co.si + bindings.len() as i32),
                    Some(new_env),
                    None,
                    None,
                    None,
                ),
                com,
            ));
        }
        Expr::If(c, t, e) => {
            // Else and endif have same label index
            let else_label = com.label("else");
            let end_if_label = com.label("end_if");
            com.index_used();

            // Use Rax
            instrs.extend(compile_expr(
                c,
                // If condition is not a tail position
                &co.modify_target(None).modify_tail(false),
                com,
            ));
            // If
            if com.result_is_bool.is_none() || com.result_is_bool.unwrap() {
                instrs.push(Cmp(ToReg(Rax, FALSE)));
                instrs.push(JumpI(Jump::E(else_label.clone())));
                // Then
                instrs.extend(compile_expr(t, co, com));
                instrs.push(JumpI(Jump::U(end_if_label.clone())));
                // Else
                instrs.push(LabelI(else_label));
                instrs.extend(compile_expr(e, co, com));

                instrs.push(LabelI(end_if_label));
            } else {
                // Then
                instrs.extend(compile_expr(t, co, com));
            }
        }
        Expr::Set(x, e) => {
            // Set expression is not a tail position
            let co = &co.modify_tail(false);
            instrs.extend(compile_expr(e, &co.modify_target(None), com));

            let venv = if co.env.contains_key(x) {
                &co.env
            } else if com.env.contains_key(x) {
                let old_var = com.env.get(x).unwrap();
                com.env.insert(
                    x.to_string(),
                    VarEnv {
                        offset: old_var.offset,
                        is_bool: com.result_is_bool,
                        in_heap: old_var.in_heap,
                    },
                );
                &com.env
            } else {
                panic!("Unbound variable identifier {x}")
            }
            .get(x)
            .unwrap();

            let reg = if venv.in_heap {
                instrs.push(Mov(ToReg(Rbx, Imm64(co.get_heap()))));
                Rbx
            } else {
                Rsp
            };

            instrs.push(Mov(ToMem(
                MemRef {
                    reg,
                    offset: venv.offset,
                },
                OReg(Rax),
            )));
            co.rax_to_target(&mut instrs)
        }
        Expr::Block(es) => {
            let block_com = &mut com.clone();
            // All variables go to mutable env
            for (k, v) in co.env.iter() {
                block_com.env.insert(k.clone(), v.clone());
            }

            // Only last expression in the block can be a tail position
            let block_co = &co.modify_env(hashmap! {});
            let block_co_rax = &block_co.modify_target(None).modify_tail(false);

            // Only last instruction needs to be put into target
            for (i, e) in es.into_iter().enumerate() {
                instrs.extend(compile_expr(
                    e,
                    if i + 1 == es.len() {
                        // Last expression
                        block_co
                    } else {
                        block_co_rax
                    },
                    block_com,
                ));
            }

            // Copy mut env vars and other stuff back
            com.update_from(&block_com);
        }
        Expr::Loop(e) => {
            // Begin and end label have same label index
            let begin_loop = com.label("begin_loop");
            let end_loop = com.label("end_loop");
            com.index_used();
            instrs.push(LabelI(begin_loop.clone()));
            // Work with Rax, move to target at the end
            instrs.extend(compile_expr(
                e,
                &com.new_ce_label(&co.modify_target(None), end_loop.clone()),
                com,
            ));
            instrs.push(JumpI(Jump::U(begin_loop)));
            instrs.push(LabelI(end_loop));
            co.rax_to_target(&mut instrs);
        }
        Expr::Break(e) => {
            // TODO: Optimize this further?
            let co = &co.modify_tail(false);
            if co.label.name == "" {
                panic!("dangling break");
            } else {
                instrs.extend(compile_expr(e, co, com));
                // Jump to end_loop
                instrs.push(JumpI(Jump::U(co.label.clone())));
            }
        }
        Expr::FnCall(name, args) => {
            let fenv = com
                .fns
                .get(name)
                .expect(&format!("Invalid: undefined function {name}"))
                .clone();
            if fenv.argc != args.len() as i32 {
                panic!("Invalid: mismatched argument count");
            }

            for (i, arg) in args.iter().enumerate() {
                // Result in main's stack
                instrs.extend(compile_expr(
                    arg,
                    &co.modify(
                        Some(co.si + i as i32),
                        None,
                        None,
                        Some(Some(MemRef {
                            reg: Rsp,
                            offset: co.si + i as i32,
                        })),
                        // Arguments to function calls are not tail positions
                        Some(false),
                    ),
                    com,
                ));
            }

            if co.tail {
                // Do tail call if co.tail is true
                // Move result from current function's stack to the current function's arguments
                let diff = com.depth - fenv.depth;
                // No need to copy if already at the right place
                if co.si != diff {
                    // Copy top to bottom or bottom to top depending on diff and co.si
                    let rng = if co.si > diff {
                        0..args.len() as i32
                    } else {
                        (args.len() as i32 - 1)..-1
                    };
                    for i in rng {
                        instrs.push(Mov(ToReg(
                            Rax,
                            Mem(MemRef {
                                reg: Rsp,
                                offset: co.si + i,
                            }),
                        )));
                        instrs.push(Mov(ToMem(
                            MemRef {
                                reg: Rsp,
                                offset: diff + i,
                            },
                            OReg(Rax),
                        )));
                    }
                }
                // Bring RSP to ret ptr
                instrs.push(Add(ToReg(Rsp, Imm(com.depth * 8))));
                instrs.push(JumpI(Jump::U(Label::new(Some(&format!("fun_{name}"))))))
                // Already in tail position, no need to move to target
            } else {
                // Move result from current function's stack to the callee's stack layout
                for i in 0..args.len() as i32 {
                    instrs.push(Mov(ToReg(
                        Rax,
                        Mem(MemRef {
                            reg: Rsp,
                            offset: co.si + i,
                        }),
                    )));
                    instrs.push(Mov(ToMem(
                        MemRef {
                            reg: Rsp,
                            offset: -(fenv.depth + 1) + i,
                        },
                        OReg(Rax),
                    )));
                }
                instrs.push(Call(Label::new(Some(&format!("fun_{name}")))));
                co.rax_to_target(&mut instrs);
            }
        }
        Expr::Define(_, _) => panic!("define cannot be compiled"),
        Expr::FnDefn(_, _, _) => panic!("Invalid: fn defn cannot be compiled here"),
    }
    return instrs;
}

pub fn instrs_to_string(instrs: &Vec<Instr>) -> String {
    instrs
        .iter()
        .map(|i| {
            if matches!(i, LabelI(_)) {
                format!("{i}")
            } else {
                format!(" {i}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn instrs_to_asm(
    cmds: &Vec<Instr>,
    ops: &mut dynasmrt::x64::Assembler,
    lbls: &mut HashMap<Label, DynamicLabel>,
) {
    cmds.iter().for_each(|c| {
        if let LabelI(l) = c {
            lbls.insert(l.clone(), ops.new_dynamic_label());
        }
    });
    cmds.iter().for_each(|c| c.asm(ops, lbls))
}
