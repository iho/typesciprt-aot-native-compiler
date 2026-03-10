//! AST → MLIR lowering.
//!
//! Each `lower_*` function walks a fragment of the OXC AST and emits MLIR
//! operations into the current `Block`.
//!
//! Current scope: emit a valid MLIR module for a TypeScript file that contains
//! only top-level `console.log("string literal")` calls (i.e. a Hello-World
//! program).  Everything else emits an "unimplemented" diagnostic and is
//! skipped.  The set of supported constructs will grow incrementally.

use anyhow::bail;
use anyhow::Result;
use melior::dialect::{func, llvm};
use melior::ir::attribute::{FlatSymbolRefAttribute, IntegerAttribute, StringAttribute, TypeAttribute};
use melior::ir::r#type::{FunctionType, IntegerType};
use melior::ir::{BlockLike, RegionLike};
use melior::ir::{Block, BlockRef, Identifier, Location, Module, Region, Value};
use melior::Context;
use oxc_ast::ast::{CallExpression, Expression, Program, Statement};
use std::sync::atomic::{AtomicU64, Ordering};

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
        string_counter: AtomicU64::new(0),
    };
    lowerer.declare_external_functions()?;
    lowerer.lower_main_function(program)?;

    Ok(module)
}

// ── Internal lowerer ─────────────────────────────────────────────────────────

struct Lowerer<'c, 'm> {
    ctx:    &'c Context,
    module: &'m Module<'c>,
    loc:    Location<'c>,
    string_counter: AtomicU64,
}

impl<'c, 'm> Lowerer<'c, 'm> {
    // ── Declare external functions ───────────────────────────────────────

    fn declare_external_functions(&mut self) -> Result<()> {
        // Declare: i32 puts(i8* str)
        let ptr = llvm::r#type::pointer(self.ctx, 0);
        let i32 = IntegerType::new(self.ctx, 32).into();
        let puts_type = FunctionType::new(self.ctx, &[ptr], &[i32]);

        let puts_op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, "puts"),
            TypeAttribute::new(puts_type.into()),
            Region::new(),
            &[(
                Identifier::new(self.ctx, "sym_visibility"),
                StringAttribute::new(self.ctx, "private").into(),
            )],
            self.loc,
        );
        self.module.body().append_operation(puts_op);
        Ok(())
    }

    // ── Emit the `main` function ─────────────────────────────────────────

    fn lower_main_function(&mut self, program: &Program<'_>) -> Result<()> {
        let main_type = FunctionType::new(self.ctx, &[], &[]);
        let region = Region::new();
        let block = region.append_block(Block::new(&[]));

        // Lower each top-level statement into the block.
        for stmt in &program.body {
            self.lower_statement(stmt, block)?;
        }

        // Return (no value)
        block.append_operation(func::r#return(&[], self.loc));

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

    // ── Statement lowering ───────────────────────────────────────────────

    fn lower_statement(&mut self, stmt: &Statement<'_>, block: BlockRef<'c, '_>) -> Result<()> {
        match stmt {
            Statement::ExpressionStatement(expr_stmt) => {
                self.lower_expression(&expr_stmt.expression, block)?;
            }
            _ => {
                tracing::debug!("skipping unimplemented statement kind");
            }
        }
        Ok(())
    }

    // ── Expression lowering ──────────────────────────────────────────────

    fn lower_expression(
        &mut self,
        expr: &Expression<'_>,
        block: BlockRef<'c, '_>,
    ) -> Result<()> {
        match expr {
            Expression::CallExpression(call) => {
                self.lower_call_expression(call, block)?;
            }
            _ => {
                tracing::debug!("skipping unimplemented expression");
            }
        }
        Ok(())
    }

    // ── Call expression: currently only `console.log(str)` ──────────────

    fn lower_call_expression(
        &mut self,
        call: &CallExpression<'_>,
        block: BlockRef<'c, '_>,
    ) -> Result<()> {
        // Detect `console.log`
        if let Expression::StaticMemberExpression(member) = &call.callee {
            let obj_is_console = matches!(
                &member.object,
                Expression::Identifier(id) if id.name == "console"
            );
            if obj_is_console && member.property.name == "log" {
                return self.emit_console_log(&call.arguments, block);
            }
        }
        tracing::debug!("skipping unimplemented call expression");
        Ok(())
    }

    /// Emit a `puts(str_ptr)` call for a single string-literal argument.
    fn emit_console_log(
        &mut self,
        args: &[oxc_ast::ast::Argument<'_>],
        block: BlockRef<'c, '_>,
    ) -> Result<()> {
        if args.len() != 1 {
            bail!("`console.log` with != 1 argument is not yet supported");
        }

        // Extract the string literal from the argument
        let str_val = match &args[0] {
            arg => {
                if let Some(expr) = arg.as_expression() {
                    if let Expression::StringLiteral(lit) = expr {
                        lit.value.as_str()
                    } else {
                        bail!("only string-literal arguments to console.log are supported");
                    }
                } else {
                    bail!("only string-literal arguments to console.log are supported");
                }
            }
        };

        // Create a global for the string constant (C-style null-terminated)
        let global_name = self.gen_global_string_name();
        self.emit_global_string(&global_name, str_val)?;

        // Get pointer to the global string using llvm.address_of
        let ptr = llvm::r#type::pointer(self.ctx, 0);
        let addr_op = llvm::address_of(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, &global_name),
            self.loc,
        );
        let addr_val = block.append_operation(addr_op).result(0)?;

        // Call puts(ptr)
        let i32 = IntegerType::new(self.ctx, 32).into();
        let call_op = func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, "puts"),
            &[addr_val.into()],
            &[i32],
            self.loc,
        );
        block.append_operation(call_op);

        Ok(())
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    fn emit_global_string(&mut self, name: &str, value: &str) -> Result<()> {
        // Build a null-terminated byte array
        let mut bytes: Vec<u8> = value.as_bytes().to_vec();
        bytes.push(0); // null terminator

        let i8 = IntegerType::new(self.ctx, 8).into();
        let len = bytes.len() as u32;
        let arr_type = llvm::r#type::array(i8, len);

        // Create the global with the array type and initializer
        let i8_attr = IntegerType::new(self.ctx, 8);
        let values: Vec<_> = bytes
            .iter()
            .map(|&b| IntegerAttribute::new(i8_attr, b as i64).into())
            .collect();

        let dense_attr = melior::ir::attribute::DenseElementsAttribute::new(
            melior::ir::r#type::RankedTensorType::new(&[len as u64], i8, None)?,
            &values,
        )
        .ok()
        .unwrap_or_else(|| {
            // Fallback: create without initializer
            melior::ir::attribute::DenseElementsAttribute::new(
                melior::ir::r#type::RankedTensorType::new(&[1], i8, None).unwrap(),
                &[IntegerAttribute::new(i8_attr, 0).into()],
            )
            .unwrap()
        });

        // llvm.mlir.global internal constant @__str_0 = dense<[...]> : !llvm.array<N x i8>
        let global_op = llvm::mlir_global(
            self.ctx,
            StringAttribute::new(self.ctx, name),
            dense_attr,
            llvm::Linkage::Internal,
            false,
            false,
            Region::new(),
            self.loc,
        );

        self.module.body().append_operation(global_op);
        Ok(())
    }

    fn gen_global_string_name(&self) -> String {
        let id = self.string_counter.fetch_add(1, Ordering::Relaxed);
        format!("__str_{}", id)
    }
}
