//! AST → MLIR lowering.
//!
//! Each `lower_*` function walks a fragment of the OXC AST and emits MLIR
//! operations into the current `Block`.
//!
//! Current scope: emit a valid MLIR module that supports:
//! - Numeric literals (integers)
//! - Arithmetic operations (+, -, *, /)
//! - Simple expressions that return values
//!
//! The generated main() function returns a computed integer value.

use anyhow::bail;
use anyhow::Result;
use melior::dialect::{func, arith};
use melior::ir::attribute::{StringAttribute, IntegerAttribute};
use melior::ir::r#type::{FunctionType, IntegerType};
use melior::ir::{BlockLike, RegionLike};
use melior::ir::{Block, BlockRef, Identifier, Location, Module, Region, Value};
use melior::Context;
use oxc_ast::ast::{CallExpression, Expression, Program, Statement, VariableDeclaration, BindingPattern};
use std::collections::HashMap;

use crate::CodegenContext;

// ── Top-level entry point ────────────────────────────────────────────────────

/// Lower a parsed TypeScript `Program` into an MLIR `Module`.
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
    };
    lowerer.lower_main_function(program)?;

    Ok(module)
}

// ── Internal lowerer ─────────────────────────────────────────────────────────

struct Lowerer<'c, 'm> {
    ctx:    &'c Context,
    module: &'m Module<'c>,
    loc:    Location<'c>,
}

impl<'c, 'm> Lowerer<'c, 'm> {
    // ── Emit the `main` function (returns i32) ─────────────────────

    fn lower_main_function(&mut self, program: &Program<'_>) -> Result<()> {
        // Define main() -> i32
        let i32_type = IntegerType::new(self.ctx, 32).into();
        let main_type = FunctionType::new(self.ctx, &[], &[i32_type]);

        let region = Region::new();
        let block = region.append_block(Block::new(&[]));

        // Create a scope for variable bindings
        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();

        // Lower program statements and collect a value to return
        let const_op = block.append_operation(
            arith::constant(
                self.ctx,
                IntegerAttribute::new(i32_type, 0).into(),
                self.loc
            )
        );
        let mut result_value = const_op.result(0)?.into();

        // Lower each statement
        for stmt in &program.body {
            if let Some(val) = self.lower_statement(stmt, block, &mut scope)? {
                result_value = val;
            }
        }

        // Return the computed value
        block.append_operation(func::r#return(&[result_value.into()], self.loc));

        let op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "main"),
            melior::ir::attribute::TypeAttribute::new(main_type.into()),
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

    // ── Statement lowering ───────────────────────────────────────────────

    fn lower_statement<'b>(
        &mut self,
        stmt: &Statement<'_>,
        block: BlockRef<'c, 'b>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        match stmt {
            Statement::ExpressionStatement(expr_stmt) => {
                self.lower_expression(&expr_stmt.expression, block, scope)
            }
            Statement::VariableDeclaration(var_decl) => {
                self.lower_variable_declaration(var_decl, block, scope)
            }
            _ => {
                tracing::debug!("skipping unimplemented statement kind");
                Ok(None)
            }
        }
    }

    // ── Variable declarations ────────────────────────────────────────────────

    fn lower_variable_declaration<'b>(
        &mut self,
        var_decl: &VariableDeclaration<'_>,
        block: BlockRef<'c, 'b>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        let mut result_value = None;

        for declarator in &var_decl.declarations {
            // Extract variable name from binding pattern
            let var_name = match &declarator.id {
                BindingPattern::BindingIdentifier(binding) => {
                    binding.name.to_string()
                }
                _ => {
                    tracing::debug!("skipping non-simple binding pattern in variable declaration");
                    continue;
                }
            };

            // Lower the initializer if present
            if let Some(init) = &declarator.init {
                match self.lower_expression(init, block, scope)? {
                    Some(value) => {
                        scope.insert(var_name.clone(), value);
                        result_value = Some(value);
                        tracing::debug!("declared variable: {} = <value>", var_name);
                    }
                    None => {
                        tracing::debug!("failed to lower initializer for variable: {}", var_name);
                    }
                }
            } else {
                tracing::debug!("variable {} has no initializer", var_name);
            }
        }

        Ok(result_value)
    }

    // ── Expression lowering: returns the computed MLIR value ────────────

    fn lower_expression<'b>(
        &mut self,
        expr: &Expression<'_>,
        block: BlockRef<'c, 'b>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        match expr {
            // Match numeric literals (OXC stores them as floats internally)
            Expression::NumericLiteral(num) => {
                let value = num.value as i64;
                self.lower_numeric_literal(value, block)
            }

            Expression::BinaryExpression(binop) => {
                self.lower_binary_operation(binop, block, scope)
            }

            Expression::Identifier(ident) => {
                let var_name = ident.name.to_string();
                match scope.get(&var_name) {
                    Some(&value) => {
                        tracing::debug!("resolved variable: {}", var_name);
                        Ok(Some(value))
                    }
                    None => {
                        bail!("undefined variable: {}", var_name)
                    }
                }
            }

            Expression::CallExpression(call) => {
                self.lower_call_expression(call, block)?;
                Ok(None)
            }

            _ => {
                tracing::debug!("skipping unimplemented expression");
                Ok(None)
            }
        }
    }

    // ── Numeric literals ─────────────────────────────────────────────────

    fn lower_numeric_literal<'b>(
        &self,
        value: i64,
        block: BlockRef<'c, 'b>,
    ) -> Result<Option<Value<'c, 'b>>> {
        let i32_type = IntegerType::new(self.ctx, 32).into();
        let const_op = arith::constant(
            self.ctx,
            IntegerAttribute::new(i32_type, value).into(),
            self.loc,
        );
        let result = block.append_operation(const_op).result(0)?;
        tracing::debug!("lowered numeric literal: {}", value);
        Ok(Some(result.into()))
    }

    // ── Binary operations ────────────────────────────────────────────────

    fn lower_binary_operation<'b>(
        &mut self,
        binop: &oxc_ast::ast::BinaryExpression<'_>,
        block: BlockRef<'c, 'b>,
        scope: &mut HashMap<String, Value<'c, 'b>>,
    ) -> Result<Option<Value<'c, 'b>>> {
        use oxc_ast::ast::BinaryOperator;

        // Lower both operands
        let lhs = self.lower_expression(&binop.left, block, scope)?
            .ok_or_else(|| anyhow::anyhow!("binary operation: failed to lower left operand"))?;

        let rhs = self.lower_expression(&binop.right, block, scope)?
            .ok_or_else(|| anyhow::anyhow!("binary operation: failed to lower right operand"))?;

        let result_op = match binop.operator {
            BinaryOperator::Addition => {
                arith::addi(lhs, rhs, self.loc)
            }
            BinaryOperator::Subtraction => {
                arith::subi(lhs, rhs, self.loc)
            }
            BinaryOperator::Multiplication => {
                arith::muli(lhs, rhs, self.loc)
            }
            BinaryOperator::Division => {
                // Integer division (signed)
                arith::divsi(lhs, rhs, self.loc)
            }
            _ => {
                bail!("unsupported binary operator: {:?}", binop.operator);
            }
        };

        let result = block.append_operation(result_op).result(0)?;
        tracing::info!("lowered binary operation: {:?}", binop.operator);
        Ok(Some(result.into()))
    }

    // ── Call expressions (console.log stub) ──────────────────────────────

    fn lower_call_expression(
        &mut self,
        call: &CallExpression<'_>,
        _block: BlockRef<'c, '_>,
    ) -> Result<()> {
        // Detect `console.log`
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let obj_is_console = matches!(
                &member.object,
                Expression::Identifier(id) if id.name == "console"
            );
            if obj_is_console && member.property.name == "log" {
                tracing::info!("console.log() recognized but not yet implemented");
                return Ok(());
            }
        }
        tracing::debug!("skipping unimplemented call expression");
        Ok(())
    }
}
