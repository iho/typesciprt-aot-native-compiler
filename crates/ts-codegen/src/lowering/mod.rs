//! AST → MLIR lowering  (Alpha v0.3)
//!
//! Supported language features (cumulative):
//! - Numeric literals (i32), boolean literals (i1)
//! - Arithmetic operators (+, -, *, /)
//! - Comparison operators (<, >, <=, >=, ==, !=)
//! - Logical NOT (!), AND (&&), OR (||) – eager evaluation
//! - Unary negation (-)
//! - Pre/post increment / decrement (++, --)
//! - Variable declarations (let / const / var)
//! - Assignment expressions (=, +=, -=, *=)
//! - if / else  (with phi-node merge)
//! - while loops  (with phi-node header)
//! - for loops    (desugared to init + while)
//! - Function declarations and calls
//! - return statements

use anyhow::{bail, Result};
use melior::dialect::{arith, cf, func, llvm};
use melior::dialect::llvm::{AllocaOptions, LoadStoreOptions};
use melior::ir::attribute::{
    DenseI32ArrayAttribute, FlatSymbolRefAttribute, IntegerAttribute, StringAttribute,
    TypeAttribute,
};
use melior::ir::r#type::{FunctionType, IntegerType};
use melior::ir::{
    Attribute, Block, BlockLike, BlockRef, Identifier, Location, Module, Region,
    RegionLike, Value, ValueLike,
};
use melior::ir::operation::OperationBuilder;
use melior::Context;
use oxc_ast::ast::{
    AssignmentTarget, BinaryExpression, BindingPattern, CallExpression, Expression,
    ForStatement, ForStatementInit, Function, IfStatement, LogicalExpression, Program,
    Statement, UnaryExpression, VariableDeclaration, WhileStatement,
};
use std::collections::HashMap;

use crate::CodegenContext;

mod statements;
mod expressions;
mod literals;
mod operators;

// ── Function signature table ──────────────────────────────────────────────────

#[derive(Clone)]
struct FuncSig<'c> {
    /// MLIR types of positional parameters (all i32 for now).
    param_types: Vec<melior::ir::Type<'c>>,
    /// Return type (i32, or None for void).
    return_type: Option<melior::ir::Type<'c>>,
}

// ── Public entry point ────────────────────────────────────────────────────────

pub fn lower_program<'c>(
    cg: &'c CodegenContext,
    program: &Program<'_>,
    _file_name: &str,
) -> Result<Module<'c>> {
    let ctx = &cg.mlir;
    let loc = Location::unknown(ctx);
    let module = Module::new(loc);

    let mut lowerer = Lowerer {
        ctx,
        module: &module,
        loc,
        funcs: HashMap::new(),
        string_count: 0,
    };

    // Pass 1 – collect function signatures so calls can be emitted before
    //          the declaration is processed (hoisting).
    lowerer.collect_function_signatures(program);

    // Emit external runtime declarations (e.g. __ts_console_log_i32).
    lowerer.emit_runtime_declarations();

    // Pass 2 – lower every top-level function declaration.
    for stmt in &program.body {
        if let Statement::FunctionDeclaration(func) = stmt {
            lowerer.lower_function_declaration(func)?;
        }
    }

    // Pass 3 – lower the implicit `main` (non-function statements).
    lowerer.lower_main_function(program)?;

    Ok(module)
}

// ── Internal lowerer ──────────────────────────────────────────────────────────

struct Lowerer<'c, 'm> {
    ctx:         &'c Context,
    module:      &'m Module<'c>,
    loc:         Location<'c>,
    funcs:       HashMap<String, FuncSig<'c>>,
    string_count: usize,
}

impl<'c, 'm> Lowerer<'c, 'm> {
    // ── Type helpers ──────────────────────────────────────────────────────

    pub(super) fn i32_type(&self) -> melior::ir::Type<'c> {
        IntegerType::new(self.ctx, 32).into()
    }

    pub(super) fn i1_type(&self) -> melior::ir::Type<'c> {
        IntegerType::new(self.ctx, 1).into()
    }

    /// Widen `i1` → `i32` (zero-extend). Pass `i32` through unchanged.
    /// For non-integer types (e.g. `!llvm.ptr`), return a zero `i32` as a
    /// safe fallback so that `main` can always return an exit code.
    pub(super) fn ensure_i32<'b>(&self, val: Value<'c, 'b>, block: BlockRef<'c, 'b>) -> Result<Value<'c, 'b>> {
        let ty = val.r#type();
        if ty == self.i1_type() {
            Ok(block.append_operation(arith::extui(val, self.i32_type(), self.loc)).result(0)?.into())
        } else if ty == self.i32_type() {
            Ok(val)
        } else if ty == self.i64_type() {
            // Unbox NaN-boxed i32 by truncating the 64-bit word to its lower 32 bits.
            Ok(block.append_operation(arith::trunci(val, self.i32_type(), self.loc)).result(0)?.into())
        } else {
            // Non-integer (e.g. string pointer) – return 0 as exit code.
            Ok(block
                .append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(self.i32_type(), 0).into(),
                    self.loc,
                ))
                .result(0)?
                .into())
        }
    }


    /// Narrow any integer to `i1` (true when != 0).
    pub(super) fn ensure_i1<'b>(&self, val: Value<'c, 'b>, block: BlockRef<'c, 'b>) -> Result<Value<'c, 'b>> {
        if val.r#type() == self.i1_type() {
            return Ok(val);
        }
        let zero = block
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i32_type(), 0).into(),
                self.loc,
            ))
            .result(0)?
            .into();
        Ok(block
            .append_operation(arith::cmpi(
                self.ctx,
                arith::CmpiPredicate::Ne,
                val,
                zero,
                self.loc,
            ))
            .result(0)?
            .into())
    }
    /// Ensure the value is an `i64` (e.g. for NaN-boxed TsVal).
    pub(super) fn ensure_i64<'b>(&self, val: Value<'c, 'b>, block: BlockRef<'c, 'b>) -> Result<Value<'c, 'b>> {
        let ty = val.r#type();
        if ty == self.i64_type() {
            Ok(val)
        } else if ty == self.i32_type() {
            let extended = block.append_operation(arith::extui(val, self.i64_type(), self.loc)).result(0)?.into();
            let mask = block.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i64_type(), 0x7FF8_0006_0000_0000u64 as i64).into(),
                self.loc,
            )).result(0)?.into();
            Ok(block.append_operation(arith::ori(extended, mask, self.loc)).result(0)?.into())
        } else {
            // Boolean or existing pointer tag?
            // For now, just extend and hope for the best.
            Ok(block.append_operation(arith::extui(val, self.i64_type(), self.loc)).result(0)?.into())
        }
    }

    pub(super) fn i64_type(&self) -> melior::ir::Type<'c> {
        IntegerType::new(self.ctx, 64).into()
    }

    pub(super) fn llvm_ptr_type(&self) -> melior::ir::Type<'c> {
        llvm::r#type::pointer(self.ctx, 0)
    }

    pub(super) fn llvm_i8_array_type(&self, len: u32) -> melior::ir::Type<'c> {
        llvm::r#type::array(IntegerType::new(self.ctx, 8).into(), len)
    }

    // ── Append a terminator only when the block doesn't have one yet ──────

    pub(super) fn terminate_with_return<'b>(&self, block: BlockRef<'c, 'b>, val: Value<'c, 'b>) -> Result<()> {
        if block.terminator().is_none() {
            let wide = self.ensure_i32(val, block)?;
            block.append_operation(func::r#return(&[wide], self.loc));
        }
        Ok(())
    }

    pub(super) fn terminate_with_br<'b>(
        &self,
        block: BlockRef<'c, 'b>,
        dest: &Block<'c>,
        args: &[Value<'c, 'b>],
    ) {
        if block.terminator().is_none() {
            block.append_operation(cf::br(dest, args, self.loc));
        }
    }

    // ── Runtime external declarations ─────────────────────────────────────

    pub(super) fn emit_runtime_declarations(&mut self) {
        let i32_type  = self.i32_type();
        let i64_type  = self.i64_type();
        let ptr_type  = self.llvm_ptr_type();
        let private   = &[(
            Identifier::new(self.ctx, "sym_visibility"),
            StringAttribute::new(self.ctx, "private").into(),
        )];

        // __ts_console_log_i32(i32) -> ()
        let op_i32 = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "__ts_console_log_i32"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[i32_type], &[]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_i32);

        // __ts_console_log_str(!llvm.ptr) -> ()
        let op_str = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "__ts_console_log_str"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[ptr_type], &[]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_str);

        // ts_retain_val(i64) -> ()
        let op_retain_val = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_retain_val"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[i64_type], &[]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_retain_val);

        // ts_retain(!llvm.ptr) -> ()
        let op_retain = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_retain"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[ptr_type], &[]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_retain);

        // ts_release(!llvm.ptr, !llvm.ptr) -> ()
        let op_release = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_release"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[ptr_type, ptr_type], &[]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_release);

        // ts_release_val(i64) -> ()
        let op_release_val = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_release_val"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[i64_type], &[]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_release_val);

        // ts_obj_new() -> i64
        let op_obj_new = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_obj_new"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[], &[i64_type]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_obj_new);

        // ts_obj_get(i64, !llvm.ptr) -> i64
        let op_obj_get = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_obj_get"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[i64_type, ptr_type], &[i64_type]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_obj_get);

        // ts_obj_set(i64, !llvm.ptr, i64) -> ()
        let op_obj_set = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_obj_set"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[i64_type, ptr_type, i64_type], &[]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_obj_set);

        // ts_arr_new(i32) -> i64
        let op_arr_new = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_arr_new"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[i32_type], &[i64_type]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_arr_new);

        // ts_arr_get(i64, i32) -> i64
        let op_arr_get = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_arr_get"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[i64_type, i32_type], &[i64_type]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_arr_get);

        // ts_arr_set(i64, i32, i64) -> ()
        let op_arr_set = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_arr_set"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[i64_type, i32_type, i64_type], &[]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_arr_set);

        // ts_arr_len(i64) -> i64
        let op_arr_len = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "ts_arr_len"),
            TypeAttribute::new(FunctionType::new(self.ctx, &[i64_type], &[i64_type]).into()),
            Region::new(),
            private,
            self.loc,
        );
        self.module.body().append_operation(op_arr_len);
    }


    // ── Function signature collection (hoisting pass) ─────────────────────

    pub(super) fn collect_function_signatures(&mut self, program: &Program<'_>) {
        let i32_type = self.i32_type();
        for stmt in &program.body {
            if let Statement::FunctionDeclaration(func) = stmt {
                let Some(id) = &func.id else { continue };
                let name = id.name.to_string();
                let param_types = vec![i32_type; func.params.items.len()];
                self.funcs.insert(name, FuncSig {
                    param_types,
                    return_type: Some(i32_type),
                });
            }
        }
    }

    // ── Function declarations ─────────────────────────────────────────────

  pub  fn lower_function_declaration(&mut self, func: &Function<'_>) -> Result<()> {
        let Some(id) = &func.id else { return Ok(()) };
        let name = id.name.to_string();
        let i32_type = self.i32_type();

        // Build parameter list.
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> = func
            .params
            .items
            .iter()
            .map(|_| (i32_type, self.loc))
            .collect();
        let return_type = i32_type;
        let func_type = FunctionType::new(self.ctx, &vec![i32_type; param_specs.len()], &[return_type]);

        let region = Region::new();
        let entry = region.append_block(Block::new(&param_specs));

        // Populate scope from block arguments.
        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        for (i, param) in func.params.items.iter().enumerate() {
            if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                scope.insert(id.name.to_string(), entry.argument(i)?.into());
            }
        }

        let mut current_block = entry;
        let mut result_value: Value<'_, '_> = {
            entry
                .append_operation(arith::constant(
                    self.ctx,
                    IntegerAttribute::new(i32_type, 0).into(),
                    self.loc,
                ))
                .result(0)?
                .into()
        };

        if let Some(body) = &func.body {
            for stmt in &body.statements {
                let (val, next) = self.lower_statement(stmt, current_block, &region, &mut scope, &[])?;
                current_block = next;
                if let Some(v) = val {
                    result_value = v;
                }
            }
        }

        // ARC: Release all variables in the function scope before final return.
        for (_, v) in &scope {
            let v_i64 = self.ensure_i64(*v, current_block)?;
            current_block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[v_i64],
                &[],
                self.loc,
            ));
        }

        // Add a default return if the last block isn't terminated yet.
        self.terminate_with_return(current_block, result_value)?;

        let op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &name),
            TypeAttribute::new(func_type.into()),
            region,
            &[],
            self.loc,
        );
        self.module.body().append_operation(op);
        Ok(())
    }
}
