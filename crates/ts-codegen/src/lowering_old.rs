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

    fn i32_type(&self) -> melior::ir::Type<'c> {
        IntegerType::new(self.ctx, 32).into()
    }

    fn i1_type(&self) -> melior::ir::Type<'c> {
        IntegerType::new(self.ctx, 1).into()
    }

    /// Widen `i1` → `i32` (zero-extend). Pass `i32` through unchanged.
    /// For non-integer types (e.g. `!llvm.ptr`), return a zero `i32` as a
    /// safe fallback so that `main` can always return an exit code.
    fn ensure_i32<'b>(&self, val: Value<'c, 'b>, block: BlockRef<'c, 'b>) -> Result<Value<'c, 'b>> {
        let ty = val.r#type();
        if ty == self.i1_type() {
            Ok(block.append_operation(arith::extui(val, self.i32_type(), self.loc)).result(0)?.into())
        } else if ty == self.i32_type() {
            Ok(val)
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
    fn ensure_i1<'b>(&self, val: Value<'c, 'b>, block: BlockRef<'c, 'b>) -> Result<Value<'c, 'b>> {
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

    fn i64_type(&self) -> melior::ir::Type<'c> {
        IntegerType::new(self.ctx, 64).into()
    }

    fn llvm_ptr_type(&self) -> melior::ir::Type<'c> {
        llvm::r#type::pointer(self.ctx, 0)
    }

    fn llvm_i8_array_type(&self, len: u32) -> melior::ir::Type<'c> {
        llvm::r#type::array(IntegerType::new(self.ctx, 8).into(), len)
    }

    // ── Append a terminator only when the block doesn't have one yet ──────

    fn terminate_with_return<'b>(&self, block: BlockRef<'c, 'b>, val: Value<'c, 'b>) -> Result<()> {
        if block.terminator().is_none() {
            let wide = self.ensure_i32(val, block)?;
            block.append_operation(func::r#return(&[wide], self.loc));
        }
        Ok(())
    }

    fn terminate_with_br<'b>(
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

    fn emit_runtime_declarations(&mut self) {
        let i32_type  = self.i32_type();
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
    }

    // ── Function signature collection (hoisting pass) ─────────────────────

    fn collect_function_signatures(&mut self, program: &Program<'_>) {
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

    // ── Implicit main function ────────────────────────────────────────────

    fn lower_main_function(&mut self, program: &Program<'_>) -> Result<()> {
        let i32_type = self.i32_type();
        let main_type = FunctionType::new(self.ctx, &[], &[i32_type]);

        let region = Region::new();
        let entry = region.append_block(Block::new(&[]));
        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();

        let mut result_value: Value<'_, '_> = entry
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i32_type, 0).into(),
                self.loc,
            ))
            .result(0)?
            .into();
        let mut current_block = entry;

        for stmt in &program.body {
            // Function declarations are emitted separately; skip here.
            if matches!(stmt, Statement::FunctionDeclaration(_)) {
                continue;
            }
            let (val, next) = self.lower_statement(stmt, current_block, &region, &mut scope, &[])?;
            current_block = next;
            if let Some(v) = val {
                result_value = v;
            }
        }

        self.terminate_with_return(current_block, result_value)?;

        let op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "main"),
            TypeAttribute::new(main_type.into()),
            region,
            &[(
                Identifier::new(self.ctx, "sym_visibility"),
                StringAttribute::new(self.ctx, "public").into(),
            )],
            self.loc,
        );
        self.module.body().append_operation(op);
        Ok(())
    }

    // ── Statement lowering ────────────────────────────────────────────────

    fn lower_statement<'b>(
        &mut self,
        stmt: &Statement<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        match stmt {
            Statement::ExpressionStatement(es) => {
                let val = self.lower_expression(&es.expression, block, region, scope)?;
                Ok((val, block))
            }
            Statement::VariableDeclaration(vd) => {
                let val = self.lower_variable_declaration(vd, block, region, scope)?;
                Ok((val, block))
            }
            Statement::FunctionDeclaration(_) => Ok((None, block)), // already handled
            Statement::ReturnStatement(ret) => {
                self.lower_return_statement(ret, block, region, scope)
            }
            Statement::IfStatement(if_stmt) => {
                self.lower_if_statement(if_stmt, block, region, scope, loops)
            }
            Statement::WhileStatement(w) => {
                self.lower_while_statement(w, block, region, scope, loops)
            }
            Statement::ForStatement(f) => {
                self.lower_for_statement(f, block, region, scope, loops)
            }
            Statement::BlockStatement(bs) => {
                let mut cur = block;
                let mut last = None;
                let mut inner = scope.clone();
                for s in &bs.body {
                    let (v, nb) = self.lower_statement(s, cur, region, &mut inner, loops)?;
                    cur = nb;
                    if let Some(v) = v { last = Some(v); }
                }
                for (k, v) in &inner {
                    if scope.contains_key(k) { scope.insert(k.clone(), *v); }
                }
                Ok((last, cur))
            }
            Statement::BreakStatement(_) => {
                if let Some((_, exit_block, scope_keys)) = loops.last() {
                    let vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
                    self.terminate_with_br(block, exit_block, &vals);
                    let dead = region.append_block(Block::new(&[]));
                    Ok((None, dead))
                } else {
                    bail!("break statement outside of loop");
                }
            }
            Statement::ContinueStatement(_) => {
                if let Some((header_block, _, scope_keys)) = loops.last() {
                    let vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
                    self.terminate_with_br(block, header_block, &vals);
                    let dead = region.append_block(Block::new(&[]));
                    Ok((None, dead))
                } else {
                    bail!("continue statement outside of loop");
                }
            }
            _ => {
                tracing::debug!("skipping unimplemented statement kind");
                Ok((None, block))
            }
        }
    }

    // ── Variable declarations ─────────────────────────────────────────────

    fn lower_variable_declaration<'b>(
        &mut self,
        var_decl: &VariableDeclaration<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        let mut result = None;
        for declarator in &var_decl.declarations {
            let name = match &declarator.id {
                BindingPattern::BindingIdentifier(b) => b.name.to_string(),
                _ => { tracing::debug!("skipping non-simple binding pattern"); continue; }
            };
            if let Some(init) = &declarator.init {
                if let Some(val) = self.lower_expression(init, block, region, scope)? {
                    scope.insert(name.clone(), val);
                    result = Some(val);
                }
            }
        }
        Ok(result)
    }

    // ── Return statement ──────────────────────────────────────────────────

    fn lower_return_statement<'b>(
        &mut self,
        ret: &oxc_ast::ast::ReturnStatement<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let val = if let Some(arg) = &ret.argument {
            self.lower_expression(arg, block, region, scope)?
                .ok_or_else(|| anyhow::anyhow!("return: expression produced no value"))?
        } else {
            // `return;` → return 0
            block.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i32_type(), 0).into(),
                self.loc,
            )).result(0)?.into()
        };

        self.terminate_with_return(block, val)?;

        // Create a dead block to absorb any unreachable code after this return.
        let dead = region.append_block(Block::new(&[]));
        Ok((None, dead))
    }

    // ── Expression lowering ───────────────────────────────────────────────

    fn lower_expression<'b>(
        &mut self,
        expr: &Expression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        match expr {
            Expression::NumericLiteral(num) => {
                Ok(Some(self.lower_numeric_literal(num.value as i64, block)?))
            }
            Expression::BooleanLiteral(b) => {
                Ok(Some(self.lower_boolean_literal(b.value, block)?))
            }
            Expression::StringLiteral(s) => {
                Ok(Some(self.lower_string_literal(s.value.as_str(), block)?))
            }
            Expression::ArrayExpression(array) => {
                self.lower_array_expression(array, block, region, scope)
            }
            Expression::ComputedMemberExpression(member) => {
                self.lower_computed_member_expression(member, block, region, scope)
            }
            Expression::StaticMemberExpression(member) => {
                // arr.length  →  load the i32 length stored at slot 0
                if member.property.name == "length" {
                    let arr = self
                        .lower_expression(&member.object, block, region, scope)?
                        .ok_or_else(|| anyhow::anyhow!("arr.length: object produced no value"))?;
                    let len: Value<'c, 'b> = block
                        .append_operation(llvm::load(
                            self.ctx, arr, self.i32_type(), self.loc,
                            LoadStoreOptions::new(),
                        ))
                        .result(0)?
                        .into();
                    return Ok(Some(len));
                }
                tracing::debug!("skipping unimplemented static member expression");
                Ok(None)
            }
            Expression::BinaryExpression(binop) => {
                self.lower_binary_expression(binop, block, region, scope)
            }
            Expression::LogicalExpression(logical) => {
                self.lower_logical_expression(logical, block, region, scope)
            }
            Expression::UnaryExpression(unary) => {
                self.lower_unary_expression(unary, block, region, scope)
            }
            Expression::UpdateExpression(update) => {
                self.lower_update_expression(update, block, scope)
            }
            Expression::AssignmentExpression(assign) => {
                self.lower_assignment_expression(assign, block, region, scope)
            }
            Expression::Identifier(ident) => {
                let name = ident.name.to_string();
                match scope.get(&name) {
                    Some(&v) => Ok(Some(v)),
                    None => bail!("undefined variable: {}", name),
                }
            }
            Expression::CallExpression(call) => {
                self.lower_call_expression(call, block, region, scope)
            }
            Expression::ConditionalExpression(cond) => {
                self.lower_conditional_expression(cond, block, region, scope)
            }
            Expression::ParenthesizedExpression(pe) => {
                self.lower_expression(&pe.expression, block, region, scope)
            }
            _ => {
                tracing::debug!("skipping unimplemented expression kind");
                Ok(None)
            }
        }
    }

    // ── Literals ──────────────────────────────────────────────────────────

    fn lower_numeric_literal<'b>(&self, value: i64, block: BlockRef<'c, 'b>) -> Result<Value<'c, 'b>> {
        Ok(block
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i32_type(), value).into(),
                self.loc,
            ))
            .result(0)?
            .into())
    }

    fn lower_boolean_literal<'b>(&self, value: bool, block: BlockRef<'c, 'b>) -> Result<Value<'c, 'b>> {
        Ok(block
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i1_type(), if value { 1 } else { 0 }).into(),
                self.loc,
            ))
            .result(0)?
            .into())
    }

    // ── Binary expressions ────────────────────────────────────────────────

    fn lower_binary_expression<'b>(
        &mut self,
        binop: &BinaryExpression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        use oxc_ast::ast::BinaryOperator;

        let lhs = self.lower_expression(&binop.left, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("binary op: no left value"))?;
        let rhs = self.lower_expression(&binop.right, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("binary op: no right value"))?;

        let op = match binop.operator {
            BinaryOperator::Addition       => arith::addi(lhs, rhs, self.loc),
            BinaryOperator::Subtraction    => arith::subi(lhs, rhs, self.loc),
            BinaryOperator::Multiplication => arith::muli(lhs, rhs, self.loc),
            BinaryOperator::Division       => arith::divsi(lhs, rhs, self.loc),
            BinaryOperator::LessThan         => arith::cmpi(self.ctx, arith::CmpiPredicate::Slt, lhs, rhs, self.loc),
            BinaryOperator::GreaterThan      => arith::cmpi(self.ctx, arith::CmpiPredicate::Sgt, lhs, rhs, self.loc),
            BinaryOperator::LessEqualThan    => arith::cmpi(self.ctx, arith::CmpiPredicate::Sle, lhs, rhs, self.loc),
            BinaryOperator::GreaterEqualThan => arith::cmpi(self.ctx, arith::CmpiPredicate::Sge, lhs, rhs, self.loc),
            BinaryOperator::Equality
            | BinaryOperator::StrictEquality   => arith::cmpi(self.ctx, arith::CmpiPredicate::Eq,  lhs, rhs, self.loc),
            BinaryOperator::Inequality
            | BinaryOperator::StrictInequality => arith::cmpi(self.ctx, arith::CmpiPredicate::Ne,  lhs, rhs, self.loc),
            _ => bail!("unsupported binary operator: {:?}", binop.operator),
        };

        Ok(Some(block.append_operation(op).result(0)?.into()))
    }

    // ── Logical expressions (&& / ||) ─────────────────────────────────────

    fn lower_logical_expression<'b>(
        &mut self,
        logical: &LogicalExpression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        use oxc_ast::ast::LogicalOperator;

        let lhs = self.lower_expression(&logical.left, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("logical op: no left value"))?;
        let rhs = self.lower_expression(&logical.right, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("logical op: no right value"))?;

        let l = self.ensure_i1(lhs, block)?;
        let r = self.ensure_i1(rhs, block)?;

        let op = match logical.operator {
            LogicalOperator::And => arith::andi(l, r, self.loc),
            LogicalOperator::Or  => arith::ori(l, r, self.loc),
            _ => bail!("unsupported logical operator: {:?}", logical.operator),
        };

        Ok(Some(block.append_operation(op).result(0)?.into()))
    }

    // ── Conditional expression (? :) ──────────────────────────────────────

    fn lower_conditional_expression<'b>(
        &mut self,
        cond: &oxc_ast::ast::ConditionalExpression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        let test_val = self.lower_expression(&cond.test, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("conditional ?: no test value"))?;
        let test_i1 = self.ensure_i1(test_val, block)?;

        // For simplicity and matching current eager logical op behavior,
        // we eagerly evaluate both branches and use `arith::select`.
        // A true short-circuiting ?: would require a block split like `if/else`.
        let cons_val = self.lower_expression(&cond.consequent, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("conditional ?: no consequent value"))?;
        let alt_val = self.lower_expression(&cond.alternate, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("conditional ?: no alternate value"))?;

        // Ensure both branches return the same type (i32).
        let cons_i32 = self.ensure_i32(cons_val, block)?;
        let alt_i32 = self.ensure_i32(alt_val, block)?;

        let op = arith::select(test_i1, cons_i32, alt_i32, self.loc);
        Ok(Some(block.append_operation(op).result(0)?.into()))
    }

    // ── Unary expressions ─────────────────────────────────────────────────

    fn lower_unary_expression<'b>(
        &mut self,
        unary: &UnaryExpression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        use oxc_ast::ast::UnaryOperator;

        let operand = self.lower_expression(&unary.argument, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("unary op: no operand"))?;

        match unary.operator {
            UnaryOperator::UnaryNegation => {
                let zero = self.lower_numeric_literal(0, block)?;
                Ok(Some(block.append_operation(arith::subi(zero, operand, self.loc)).result(0)?.into()))
            }
            UnaryOperator::LogicalNot => {
                let x = self.ensure_i1(operand, block)?;
                let zero_i1 = block
                    .append_operation(arith::constant(
                        self.ctx,
                        IntegerAttribute::new(self.i1_type(), 0).into(),
                        self.loc,
                    ))
                    .result(0)?
                    .into();
                Ok(Some(
                    block
                        .append_operation(arith::cmpi(self.ctx, arith::CmpiPredicate::Eq, x, zero_i1, self.loc))
                        .result(0)?
                        .into(),
                ))
            }
            _ => bail!("unsupported unary operator: {:?}", unary.operator),
        }
    }

    // ── Update expressions (i++, ++i, i--, --i) ───────────────────────────

    fn lower_update_expression<'b>(
        &mut self,
        update: &oxc_ast::ast::UpdateExpression<'_>,
        block: BlockRef<'c, 'b>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        use oxc_ast::ast::UpdateOperator;

        let name = match &update.argument {
            oxc_ast::ast::SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => id.name.to_string(),
            _ => bail!("update expression: only simple identifiers are supported"),
        };

        let old_val = *scope.get(&name)
            .ok_or_else(|| anyhow::anyhow!("undefined variable: {}", name))?;
        let one = self.lower_numeric_literal(1, block)?;

        let new_val = match update.operator {
            UpdateOperator::Increment => block.append_operation(arith::addi(old_val, one, self.loc)).result(0)?.into(),
            UpdateOperator::Decrement => block.append_operation(arith::subi(old_val, one, self.loc)).result(0)?.into(),
        };

        scope.insert(name, new_val);
        // Prefix returns new value; postfix returns old value.
        Ok(Some(if update.prefix { new_val } else { old_val }))
    }

    // ── Assignment expressions ────────────────────────────────────────────

    fn lower_assignment_expression<'b>(
        &mut self,
        assign: &oxc_ast::ast::AssignmentExpression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        use oxc_ast::ast::AssignmentOperator;

        let name = match &assign.left {
            AssignmentTarget::AssignmentTargetIdentifier(id) => id.name.to_string(),
            _ => bail!("unsupported assignment target"),
        };

        let rhs = self.lower_expression(&assign.right, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("assignment: rhs produced no value"))?;

        let new_val = match assign.operator {
            AssignmentOperator::Assign => rhs,
            AssignmentOperator::Addition => {
                let lhs = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined: {}", name))?;
                block.append_operation(arith::addi(lhs, rhs, self.loc)).result(0)?.into()
            }
            AssignmentOperator::Subtraction => {
                let lhs = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined: {}", name))?;
                block.append_operation(arith::subi(lhs, rhs, self.loc)).result(0)?.into()
            }
            AssignmentOperator::Multiplication => {
                let lhs = *scope.get(&name).ok_or_else(|| anyhow::anyhow!("undefined: {}", name))?;
                block.append_operation(arith::muli(lhs, rhs, self.loc)).result(0)?.into()
            }
            _ => bail!("unsupported compound assignment operator"),
        };

        scope.insert(name, new_val);
        Ok(Some(new_val))
    }

    // ── If / else  (phi-node merge) ───────────────────────────────────────

    fn lower_if_statement<'b>(
        &mut self,
        if_stmt: &IfStatement<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let cond_val = self
            .lower_expression(&if_stmt.test, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("if condition must produce a value"))?;
        let cond_i1 = self.ensure_i1(cond_val, block)?;

        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        let merge_arg_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|k| (scope[k].r#type(), self.loc)).collect();

        let then_block  = region.append_block(Block::new(&[]));
        let else_block  = region.append_block(Block::new(&[]));
        let merge_block = region.append_block(Block::new(&merge_arg_types));

        block.append_operation(cf::cond_br(
            self.ctx, cond_i1, &then_block, &else_block, &[], &[], self.loc,
        ));

        // Then branch
        let mut then_scope = scope.clone();
        let (_, then_end) = self.lower_statement(&if_stmt.consequent, then_block, region, &mut then_scope, loops)?;
        let then_vals: Vec<Value<'c, 'b>> =
            scope_keys.iter().map(|k| *then_scope.get(k).unwrap_or(&scope[k])).collect();
        self.terminate_with_br(then_end, &merge_block, &then_vals);

        // Else branch
        let mut else_scope = scope.clone();
        if let Some(alt) = &if_stmt.alternate {
            let (_, else_end) = self.lower_statement(alt, else_block, region, &mut else_scope, loops)?;
            let else_vals: Vec<Value<'c, 'b>> =
                scope_keys.iter().map(|k| *else_scope.get(k).unwrap_or(&scope[k])).collect();
            self.terminate_with_br(else_end, &merge_block, &else_vals);
        } else {
            let orig_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
            self.terminate_with_br(else_block, &merge_block, &orig_vals);
        }

        // Update scope to use merge-block phi arguments.
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), merge_block.argument(i)?.into());
        }

        Ok((None, merge_block))
    }

    // ── While loop  (phi-node header) ────────────────────────────────────

    fn lower_while_statement<'b>(
        &mut self,
        while_stmt: &WhileStatement<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        let phi_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|k| (scope[k].r#type(), self.loc)).collect();

        // header receives all scope vars as block arguments (loop-carried values).
        let header_block = region.append_block(Block::new(&phi_types));
        let body_block   = region.append_block(Block::new(&[]));
        let exit_block   = region.append_block(Block::new(&phi_types));

        // Jump into the header with initial values.
        let init_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
        block.append_operation(cf::br(&header_block, &init_vals, self.loc));

        // Build scope for the header (use block arguments).
        let mut header_scope = scope.clone();
        for (i, k) in scope_keys.iter().enumerate() {
            header_scope.insert(k.clone(), header_block.argument(i)?.into());
        }

        // Evaluate condition inside the header.
        let cond_val = self
            .lower_expression(&while_stmt.test, header_block, region, &mut header_scope)?
            .ok_or_else(|| anyhow::anyhow!("while condition must produce a value"))?;
        let cond_i1 = self.ensure_i1(cond_val, header_block)?;

        // The exit block gets the header-block values when the condition is false.
        let header_vals: Vec<Value<'c, 'b>> = (0..scope_keys.len())
            .map(|i| header_block.argument(i).map(Into::into))
            .collect::<Result<_, _>>()?;
        header_block.append_operation(cf::cond_br(
            self.ctx, cond_i1, &body_block, &exit_block, &[], &header_vals, self.loc,
        ));

        // Lower the loop body.
        let mut body_scope = header_scope.clone();
        let mut inner_loops = loops.to_vec();
        inner_loops.push((header_block, exit_block, scope_keys.clone()));
        let (_, body_end) =
            self.lower_statement(&while_stmt.body, body_block, region, &mut body_scope, &inner_loops)?;
        let body_vals: Vec<Value<'c, 'b>> =
            scope_keys.iter().map(|k| *body_scope.get(k).unwrap_or(&header_scope[k])).collect();
        self.terminate_with_br(body_end, &header_block, &body_vals);

        // After the loop, scope uses exit-block arguments.
        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), exit_block.argument(i)?.into());
        }

        Ok((None, exit_block))
    }

    // ── For loop (desugared: init + while) ───────────────────────────────

    fn lower_for_statement<'b>(
        &mut self,
        for_stmt: &ForStatement<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
        loops: &[(BlockRef<'c, 'b>, BlockRef<'c, 'b>, Vec<String>)],
    ) -> Result<(Option<Value<'c, 'b>>, BlockRef<'c, 'b>)> {
        // Lower init (may introduce new variables into scope).
        let mut current = block;
        if let Some(init) = &for_stmt.init {
            match init {
                ForStatementInit::VariableDeclaration(vd) => {
                    self.lower_variable_declaration(vd, current, region, scope)?;
                }
                _ => {
                    // Treat as an expression (ForStatementInit inherits Expression variants).
                    let expr = init.as_expression().ok_or_else(|| {
                        anyhow::anyhow!("unsupported for-loop init")
                    })?;
                    self.lower_expression(expr, current, region, scope)?;
                }
            }
        }

        let scope_keys: Vec<String> = scope.keys().cloned().collect();
        let phi_types: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            scope_keys.iter().map(|k| (scope[k].r#type(), self.loc)).collect();

        let header_block = region.append_block(Block::new(&phi_types));
        let body_block   = region.append_block(Block::new(&[]));
        let exit_block   = region.append_block(Block::new(&phi_types));

        let init_vals: Vec<Value<'c, 'b>> = scope_keys.iter().map(|k| scope[k]).collect();
        current.append_operation(cf::br(&header_block, &init_vals, self.loc));

        // Header scope: use block arguments.
        let mut header_scope = scope.clone();
        for (i, k) in scope_keys.iter().enumerate() {
            header_scope.insert(k.clone(), header_block.argument(i)?.into());
        }

        // Evaluate condition (or default to `true` if absent).
        let cond_i1 = if let Some(test) = &for_stmt.test {
            let cv = self.lower_expression(test, header_block, region, &mut header_scope)?
                .ok_or_else(|| anyhow::anyhow!("for condition must produce a value"))?;
            self.ensure_i1(cv, header_block)?
        } else {
            self.lower_boolean_literal(true, header_block)?
        };

        let header_vals: Vec<Value<'c, 'b>> = (0..scope_keys.len())
            .map(|i| header_block.argument(i).map(Into::into))
            .collect::<Result<_, _>>()?;
        header_block.append_operation(cf::cond_br(
            self.ctx, cond_i1, &body_block, &exit_block, &[], &header_vals, self.loc,
        ));

        // Lower body, then update expression.
        let mut body_scope = header_scope.clone();
        let mut inner_loops = loops.to_vec();
        inner_loops.push((header_block, exit_block, scope_keys.clone()));
        let (_, body_end) =
            self.lower_statement(&for_stmt.body, body_block, region, &mut body_scope, &inner_loops)?;

        // Lower update in the body-end block.
        if let Some(update) = &for_stmt.update {
            if body_end.terminator().is_none() {
                self.lower_expression(update, body_end, region, &mut body_scope)?;
            }
        }

        let body_vals: Vec<Value<'c, 'b>> =
            scope_keys.iter().map(|k| *body_scope.get(k).unwrap_or(&header_scope[k])).collect();
        self.terminate_with_br(body_end, &header_block, &body_vals);

        for (i, k) in scope_keys.iter().enumerate() {
            scope.insert(k.clone(), exit_block.argument(i)?.into());
        }

        Ok((None, exit_block))
    }

    // ── Array literals ────────────────────────────────────────────────────

    /// Lower `[e0, e1, …]` to a stack-allocated i32 array.
    ///
    /// Layout in memory:  `[i32 length | i32 e0 | i32 e1 | …]`
    /// The returned value is a `!llvm.ptr` to the first element (the length).
    fn lower_array_expression<'b>(
        &mut self,
        array: &oxc_ast::ast::ArrayExpression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        let i32_type = self.i32_type();
        let ptr_type = self.llvm_ptr_type();

        let n = array.elements.len();
        let total = (1 + n) as i64; // 1 slot for length + n element slots

        // Allocate `total` i32s on the stack.
        let size_val: Value<'c, 'b> = block
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i32_type, total).into(),
                self.loc,
            ))
            .result(0)?
            .into();

        let arr_ptr: Value<'c, 'b> = block
            .append_operation(llvm::alloca(
                self.ctx,
                size_val,
                ptr_type,
                self.loc,
                AllocaOptions::new().elem_type(Some(TypeAttribute::new(i32_type))),
            ))
            .result(0)?
            .into();

        // Store length at index 0 (arr_ptr points there directly).
        let len_val: Value<'c, 'b> = block
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i32_type, n as i64).into(),
                self.loc,
            ))
            .result(0)?
            .into();
        block.append_operation(llvm::store(self.ctx, len_val, arr_ptr, self.loc, LoadStoreOptions::new()));

        // Store each element at index 1+i.
        for (i, elem) in array.elements.iter().enumerate() {
            let Some(expr) = elem.as_expression() else { continue };
            let val = self.lower_expression(expr, block, region, scope)?
                .ok_or_else(|| anyhow::anyhow!("array element produced no value"))?;
            let val_i32 = self.ensure_i32(val, block)?;

            let elem_ptr: Value<'c, 'b> = block
                .append_operation(llvm::get_element_ptr(
                    self.ctx,
                    arr_ptr,
                    DenseI32ArrayAttribute::new(self.ctx, &[(1 + i) as i32]),
                    i32_type,
                    ptr_type,
                    self.loc,
                ))
                .result(0)?
                .into();
            block.append_operation(llvm::store(self.ctx, val_i32, elem_ptr, self.loc, LoadStoreOptions::new()));
        }

        Ok(Some(arr_ptr))
    }

    /// Lower `arr[idx]` to an i32 load.
    fn lower_computed_member_expression<'b>(
        &mut self,
        member: &oxc_ast::ast::ComputedMemberExpression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        let i32_type = self.i32_type();
        let ptr_type = self.llvm_ptr_type();

        let arr = self
            .lower_expression(&member.object, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("array: object expression produced no value"))?;

        let idx = self
            .lower_expression(&member.expression, block, region, scope)?
            .ok_or_else(|| anyhow::anyhow!("array: index expression produced no value"))?;
        let idx_i32 = self.ensure_i32(idx, block)?;

        // Offset by 1 to skip the length prefix stored at slot 0.
        let one: Value<'c, 'b> = block
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i32_type, 1).into(),
                self.loc,
            ))
            .result(0)?
            .into();
        let actual_idx: Value<'c, 'b> = block
            .append_operation(arith::addi(idx_i32, one, self.loc))
            .result(0)?
            .into();

        let elem_ptr: Value<'c, 'b> = block
            .append_operation(llvm::get_element_ptr_dynamic(
                self.ctx,
                arr,
                &[actual_idx],
                i32_type,
                ptr_type,
                self.loc,
            ))
            .result(0)?
            .into();

        let val: Value<'c, 'b> = block
            .append_operation(llvm::load(self.ctx, elem_ptr, i32_type, self.loc, LoadStoreOptions::new()))
            .result(0)?
            .into();

        Ok(Some(val))
    }

    // ── String literals ───────────────────────────────────────────────────

    /// Emit a `llvm.mlir.global` for the string (with null terminator) at
    /// module level, then return a `!llvm.ptr` pointing to the first byte.
    fn lower_string_literal<'b>(
        &mut self,
        s: &str,
        block: BlockRef<'c, 'b>,
    ) -> Result<Value<'c, 'b>> {
        let name = format!("__ts_str_{}", self.string_count);
        self.string_count += 1;

        // Build null-terminated byte slice and treat it as a &str for MLIR
        // (mlirStringAttrGet uses (ptr, len) so embedded nulls are fine).
        let mut bytes = s.as_bytes().to_vec();
        bytes.push(0u8);
        let len = bytes.len() as u32;
        // SAFETY: MLIR receives (ptr, len), so any bytes are valid here.
        let content = unsafe { std::str::from_utf8_unchecked(&bytes) };

        let array_type = self.llvm_i8_array_type(len);
        let ptr_type   = self.llvm_ptr_type();
        let i32_type   = self.i32_type();

        let linkage  = Attribute::parse(self.ctx, "#llvm.linkage<internal>")
            .ok_or_else(|| anyhow::anyhow!("failed to parse #llvm.linkage<internal>"))?;
        let unit_attr = Attribute::parse(self.ctx, "unit")
            .ok_or_else(|| anyhow::anyhow!("failed to parse unit attribute"))?;

        // llvm.mlir.global internal constant @__ts_str_N("<bytes>") : !llvm.array<N x i8>
        // The op always requires exactly one region (empty = attribute initializer).
        let global_op = OperationBuilder::new("llvm.mlir.global", self.loc)
            .add_attributes(&[
                (Identifier::new(self.ctx, "sym_name"),    StringAttribute::new(self.ctx, &name).into()),
                (Identifier::new(self.ctx, "global_type"), TypeAttribute::new(array_type).into()),
                (Identifier::new(self.ctx, "linkage"),     linkage),
                (Identifier::new(self.ctx, "value"),       StringAttribute::new(self.ctx, content).into()),
                (Identifier::new(self.ctx, "addr_space"),  IntegerAttribute::new(i32_type, 0).into()),
                (Identifier::new(self.ctx, "constant"),    unit_attr),
            ])
            .add_regions([Region::new()])
            .build()?;
        self.module.body().append_operation(global_op);

        // %arr_ptr = llvm.mlir.addressof @__ts_str_N : !llvm.ptr
        let addr_op = OperationBuilder::new("llvm.mlir.addressof", self.loc)
            .add_attributes(&[(
                Identifier::new(self.ctx, "global_name"),
                FlatSymbolRefAttribute::new(self.ctx, &name).into(),
            )])
            .add_results(&[ptr_type])
            .build()?;
        let arr_ptr: Value<'c, 'b> = block.append_operation(addr_op).result(0)?.into();

        // %char_ptr = llvm.getelementptr inbounds %arr_ptr[0, 0] : (!llvm.ptr) -> !llvm.ptr
        let char_ptr: Value<'c, 'b> = block
            .append_operation(llvm::get_element_ptr(
                self.ctx,
                arr_ptr,
                DenseI32ArrayAttribute::new(self.ctx, &[0, 0]),
                array_type,
                ptr_type,
                self.loc,
            ))
            .result(0)?
            .into();

        Ok(char_ptr)
    }

    // ── Call expressions ──────────────────────────────────────────────────

    fn lower_call_expression<'b>(
        &mut self,
        call: &CallExpression<'_>,
        block: BlockRef<'c, 'b>,
        region: &'b Region<'c>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        // console.log(x) → __ts_console_log_i32(x) or __ts_console_log_str(x)
        if let Expression::StaticMemberExpression(member) = &call.callee {
            if matches!(&member.object, Expression::Identifier(id) if id.name == "console")
                && member.property.name == "log"
            {
                if let Some(first_arg) = call.arguments.first() {
                    let expr = first_arg.as_expression()
                        .ok_or_else(|| anyhow::anyhow!("console.log: spread argument not supported"))?;
                    let val = self.lower_expression(expr, block, region, scope)?
                        .ok_or_else(|| anyhow::anyhow!("console.log: argument produced no value"))?;
                    if val.r#type() == self.llvm_ptr_type() {
                        // String argument → __ts_console_log_str(ptr)
                        block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "__ts_console_log_str"),
                            &[val],
                            &[],
                            self.loc,
                        ));
                    } else {
                        // Numeric/bool argument → __ts_console_log_i32(n)
                        let val_i32 = self.ensure_i32(val, block)?;
                        block.append_operation(func::call(
                            self.ctx,
                            FlatSymbolRefAttribute::new(self.ctx, "__ts_console_log_i32"),
                            &[val_i32],
                            &[],
                            self.loc,
                        ));
                    }
                }
                return Ok(None);
            }
        }

        // User-defined function call
        if let Expression::Identifier(callee_id) = &call.callee {
            let name = callee_id.name.to_string();
            if let Some(sig) = self.funcs.get(&name).cloned() {
                // Lower arguments.
                let mut args: Vec<Value<'c, 'b>> = Vec::new();
                for arg in &call.arguments {
                    let expr = arg.as_expression()
                        .ok_or_else(|| anyhow::anyhow!("spread in function call not supported"))?;
                    let v = self.lower_expression(expr, block, region, scope)?
                        .ok_or_else(|| anyhow::anyhow!("argument produced no value"))?;
                    args.push(v);
                }

                let result_types: Vec<melior::ir::Type<'c>> =
                    sig.return_type.iter().copied().collect();

                let op = block.append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, &name),
                    &args,
                    &result_types,
                    self.loc,
                ));

                return if sig.return_type.is_some() {
                    Ok(Some(op.result(0)?.into()))
                } else {
                    Ok(None)
                };
            }
        }

        tracing::debug!("skipping unimplemented call expression");
        Ok(None)
    }
}
