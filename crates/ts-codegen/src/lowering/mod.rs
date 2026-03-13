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
use melior::ir::{
    attribute::{
        DenseI32ArrayAttribute, FlatSymbolRefAttribute, IntegerAttribute, StringAttribute,
        TypeAttribute,
    },
    r#type::{FunctionType, IntegerType},
    Attribute, Block, BlockLike, BlockRef, Identifier, Location, Module, Region,
    RegionLike, Type, Value, ValueLike,
};
use melior::ir::operation::OperationBuilder;
use melior::Context;
use oxc_ast::ast::{
    ArrowFunctionExpression, ArrayPattern, AssignmentTarget, BinaryExpression, BindingPattern,
    BindingProperty, CallExpression, CatchClause, ChainElement, ChainExpression, Class, ClassBody,
    ClassElement, Declaration, Expression, ExportDefaultDeclaration, ExportNamedDeclaration,
    ForInStatement, ForOfStatement, ForStatement, ForStatementInit, ForStatementLeft, Function,
    IfStatement, ImportDeclaration, ImportDeclarationSpecifier,
    LogicalExpression, MethodDefinition, MethodDefinitionKind, NewExpression, ObjectPattern,
    PrivateFieldExpression, Program, PropertyDefinition, Statement, TemplateLiteral,
    ThisExpression, ThrowStatement, TSAsExpression, TSEnumDeclaration, TSSatisfiesExpression,
    TSTypeAssertion, TryStatement, UnaryExpression, VariableDeclaration, WhileStatement,
};
use std::collections::HashMap;

use crate::CodegenContext;

mod statements;
mod expressions;
mod literals;
mod operators;
mod classes;
mod enums;

// ── Function signature table ──────────────────────────────────────────────────

#[derive(Clone)]
struct FuncSig<'c> {
    /// MLIR types of positional parameters (all i64).
    #[allow(dead_code)]
    param_types: Vec<melior::ir::Type<'c>>,
    /// Return type (i64, or None for void).
    return_type: Option<melior::ir::Type<'c>>,
    /// True if the last MLIR param is a rest array (i64 TsArray).
    has_rest: bool,
}
 
#[derive(Clone)]
struct ClassSig {
    constructor_name: String,
    methods:  HashMap<String, String>,                  // instance method name → mangled
    statics:  HashMap<String, String>,                  // static method name → mangled
    getters:  std::collections::HashSet<String>,        // property names with getters
    setters:  std::collections::HashSet<String>,        // property names with setters
    parent:   Option<String>,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Parse a local import file and return its Program with 'static lifetime.
/// Uses Box::leak so the allocator lives for the process lifetime (acceptable for a compiler).
fn load_import_static(path: &std::path::Path) -> Option<oxc_ast::ast::Program<'static>> {
    let source = std::fs::read_to_string(path).ok()?;
    let source: &'static str = Box::leak(source.into_boxed_str());
    let alloc: &'static oxc_allocator::Allocator =
        Box::leak(Box::new(oxc_allocator::Allocator::default()));
    ts_frontend::parse_typescript(alloc, source, &path.display().to_string()).ok()
}

/// Collect local import paths declared in a parsed program, relative to `base_dir`.
fn collect_local_imports(
    program: &oxc_ast::ast::Program<'_>,
    base_dir: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    use oxc_ast::ast::ImportOrExportKind;
    let mut paths = Vec::new();
    for stmt in &program.body {
        if let Statement::ImportDeclaration(import) = stmt {
            if import.import_kind == ImportOrExportKind::Type { continue; }
            let src = import.source.value.as_str();
            if src.starts_with("./") || src.starts_with("../") {
                let mut p = base_dir.join(src);
                if p.extension().is_none() {
                    p.set_extension("ts");
                } else if p.extension().map_or(false, |e| e != "ts") {
                    let ts = p.with_extension("ts");
                    if ts.exists() { p = ts; }
                }
                paths.push(p);
            }
        }
    }
    paths
}

/// Recursively load and lower a local import file and all its transitive dependencies.
/// `visited` prevents processing the same file twice.
fn process_import_recursive<'c, 'm>(
    lowerer: &mut Lowerer<'c, 'm>,
    path: &std::path::Path,
    visited: &mut std::collections::HashSet<std::path::PathBuf>,
) -> Result<()> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if visited.contains(&canonical) { return Ok(()); }
    visited.insert(canonical);

    let Some(imported) = load_import_static(path) else {
        tracing::warn!("failed to resolve import: {}", path.display());
        return Ok(());
    };

    // Process transitive imports depth-first.
    let base_dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    for sub_path in collect_local_imports(&imported, base_dir) {
        process_import_recursive(lowerer, &sub_path, visited)?;
    }

    // Register signatures and lower declarations for this file.
    lowerer.collect_function_signatures(&imported);
    lowerer.collect_class_definitions(&imported);
    lowerer.collect_enum_definitions(&imported);
    for stmt in &imported.body {
        match stmt {
            Statement::ClassDeclaration(class) => {
                lowerer.lower_class_declaration(class)?;
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(Declaration::ClassDeclaration(class)) = &exp.declaration {
                    lowerer.lower_class_declaration(class)?;
                }
            }
            _ => {}
        }
    }
    for stmt in &imported.body {
        match stmt {
            Statement::FunctionDeclaration(func) => {
                lowerer.lower_function_declaration(func)?;
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(Declaration::FunctionDeclaration(func)) = &exp.declaration {
                    lowerer.lower_function_declaration(func)?;
                }
            }
            Statement::ExportDefaultDeclaration(exp) => {
                use oxc_ast::ast::ExportDefaultDeclarationKind;
                if let ExportDefaultDeclarationKind::FunctionDeclaration(func) = &exp.declaration {
                    lowerer.lower_function_declaration(func)?;
                }
            }
            _ => {}
        }
    }
    lowerer.lower_module_const_functions(&imported)?;
    Ok(())
}

pub fn lower_program<'c>(
    cg: &'c CodegenContext,
    program: &Program<'_>,
    file_name: &str,
) -> Result<Module<'c>> {
    let ctx = &cg.mlir;
    let loc = Location::unknown(ctx);
    let module = Module::new(loc);

    let i32_type = IntegerType::new(ctx, 32).into();
    let mut lowerer = Lowerer {
        ctx,
        module: &module,
        loc,
        funcs: HashMap::new(),
        classes: HashMap::new(),
        string_count: 0,
        arrow_count: 0,
        var_class_types: HashMap::new(),
        fn_return_type: i32_type,
        is_async: false,
        enums: HashMap::new(),
        current_class: None,
        super_ctor: None,
        builtin_aliases: HashMap::new(),
        module_global_names: std::collections::HashSet::new(),
        builtin_wrappers_emitted: std::collections::HashSet::new(),
        lowered_classes: std::collections::HashSet::new(),
    };

    // Emit external runtime declarations (e.g. __ts_console_log_i32).
    lowerer.emit_runtime_declarations();

    // Pre-pass: resolve and inline local imports.
    let base_dir = std::path::Path::new(file_name)
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));

    // Collect all local import paths first (avoid borrow issues)
    let mut local_imports: Vec<(std::path::PathBuf, Vec<String>)> = Vec::new();
    for stmt in &program.body {
        if let Statement::ImportDeclaration(import) = stmt {
            use oxc_ast::ast::ImportOrExportKind;
            // Skip `import type { ... }` — these are type-only and have no runtime effect.
            if import.import_kind == ImportOrExportKind::Type { continue; }
            let src = import.source.value.as_str();
            if src.starts_with("./") || src.starts_with("../") {
                // Resolve .ts extension
                let mut path = base_dir.join(src);
                if path.extension().is_none() {
                    path.set_extension("ts");
                } else if path.extension().map_or(false, |e| e != "ts") {
                    // Non-ts import (e.g., .js), try adding .ts
                    let ts_path = path.with_extension("ts");
                    if ts_path.exists() { path = ts_path; }
                }
                // Collect imported names
                let names: Vec<String> = if let Some(specs) = &import.specifiers {
                    specs.iter().filter_map(|spec| {
                        match spec {
                            ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                Some(s.local.name.to_string())
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                Some(s.local.name.to_string())
                            }
                            _ => None,
                        }
                    }).collect()
                } else {
                    Vec::new()
                };
                local_imports.push((path, names));
            }
            // External imports (node:, npm packages) are silently skipped.
        }
    }

    // Process all local imports recursively (handles transitive dependencies).
    let mut visited: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    for (import_path, _names) in &local_imports {
        process_import_recursive(&mut lowerer, import_path, &mut visited)?;
    }

    // Pass 1 – collect function signatures and class definitions.
    lowerer.collect_function_signatures(program);
    lowerer.collect_class_definitions(program);
    lowerer.collect_enum_definitions(program);

    // Pass 2a – lower class declarations (constructors + methods).
    // TODO: Apply class decorators: @dec class Foo {} → Foo = dec(Foo)
    // Decorators require first-class function support, not yet implemented.
    for stmt in &program.body {
        match stmt {
            Statement::ClassDeclaration(class) => {
                lowerer.lower_class_declaration(class)?;
            }
            Statement::ExportNamedDeclaration(export) => {
                if let Some(Declaration::ClassDeclaration(class)) = &export.declaration {
                    lowerer.lower_class_declaration(class)?;
                }
            }
            _ => {}
        }
    }

    // Pass 2b – lower every top-level function declaration.
    for stmt in &program.body {
        match stmt {
            Statement::FunctionDeclaration(func) => {
                lowerer.lower_function_declaration(func)?;
            }
            Statement::ExportNamedDeclaration(export) => {
                if let Some(Declaration::FunctionDeclaration(func)) = &export.declaration {
                    lowerer.lower_function_declaration(func)?;
                }
            }
            Statement::ExportDefaultDeclaration(export) => {
                use oxc_ast::ast::ExportDefaultDeclarationKind;
                if let ExportDefaultDeclarationKind::FunctionDeclaration(func) = &export.declaration {
                    lowerer.lower_function_declaration(func)?;
                }
            }
            _ => {}
        }
    }

    // Pass 2c – lower module-level const arrow/function declarations as hoisted functions.
    lowerer.lower_module_const_functions(program)?;

    // Pass 3 – lower the implicit `main` (non-function statements).
    lowerer.lower_main_function(program)?;

    Ok(module)
}

// ── Internal lowerer ──────────────────────────────────────────────────────────

struct Lowerer<'c, 'm> {
    ctx:            &'c Context,
    module:         &'m Module<'c>,
    loc:            Location<'c>,
    funcs:          HashMap<String, FuncSig<'c>>,
    classes:        HashMap<String, ClassSig>,
    string_count:   usize,
    /// Counter for generating unique arrow-function names.
    arrow_count:    usize,
    /// Maps variable name → class name for `new Foo()` assignments.
    /// Used for method-call dispatch without full type inference.
    var_class_types: HashMap<String, String>,
    /// Return type of the function currently being lowered.
    /// `i32` for regular functions/main; `i64` for class methods.
    fn_return_type: melior::ir::Type<'c>,
    /// Whether the function currently being lowered is `async`.
    is_async: bool,
    /// Maps enum name → (member name → integer value) for compile-time resolution.
    enums: HashMap<String, HashMap<String, i64>>,
    current_class: Option<String>,
    super_ctor: Option<String>,
    /// Maps `const alias = builtin` aliases (e.g. `decodeURIComponent_` → `decodeURIComponent`).
    /// Used to redirect calls to aliased built-in functions.
    builtin_aliases: HashMap<String, String>,
    /// Module-level non-function const names (e.g. `patternCache = {}`).
    /// These are initialized in `main` via `ts_set_module_global` and retrieved
    /// via `ts_get_module_global` at the start of every module-level function.
    module_global_names: std::collections::HashSet<String>,
    /// Tracks which built-in wrapper MLIR functions have already been emitted.
    builtin_wrappers_emitted: std::collections::HashSet<String>,
    /// Tracks which classes have already been lowered (to prevent re-emission from duplicate imports).
    lowered_classes: std::collections::HashSet<String>,
}

impl<'c, 'm> Lowerer<'c, 'm> {
    // ── Built-in wrapper functions ─────────────────────────────────────────

    /// Returns (wrapper_fn_name, arity, [runtime_fn_name]) for a known JS built-in function.
    /// Used when the built-in is referenced as a first-class value (not called directly).
    fn builtin_wrapper_info(name: &str) -> Option<(&'static str, usize, &'static str)> {
        match name {
            "decodeURI"           => Some(("__wrap_decodeURI", 1, "ts_decode_uri")),
            "decodeURIComponent"  => Some(("__wrap_decodeURIComponent", 1, "ts_decode_uri_component")),
            "encodeURI"           => Some(("__wrap_encodeURI", 1, "ts_encode_uri")),
            "encodeURIComponent"  => Some(("__wrap_encodeURIComponent", 1, "ts_encode_uri_component")),
            "parseInt"            => Some(("__wrap_parseInt", 2, "ts_parse_int")),
            "parseFloat"          => Some(("__wrap_parseFloat", 1, "ts_parse_float")),
            "Number"              => Some(("__wrap_Number", 1, "ts_coerce_number")),
            "String"              => Some(("__wrap_String", 1, "ts_coerce_string")),
            _ => None,
        }
    }

    /// Emit a wrapper MLIR function for a built-in, if not already emitted.
    /// The wrapper has the closure calling convention: (env: i64, arg0: i64, ...) -> i64.
    pub(super) fn ensure_builtin_wrapper(&mut self, js_name: &str) -> Result<Option<String>> {
        let Some((wrapper_name, arity, runtime_fn)) = Self::builtin_wrapper_info(js_name) else {
            return Ok(None);
        };
        if self.builtin_wrappers_emitted.contains(wrapper_name) {
            return Ok(Some(wrapper_name.to_string()));
        }
        self.builtin_wrappers_emitted.insert(wrapper_name.to_string());

        let i64t = self.i64_type();
        // Params: env (i64) + arity regular params (i64 each)
        let n_mlir_params = 1 + arity;
        let param_specs: Vec<(Type<'c>, Location<'c>)> =
            (0..n_mlir_params).map(|_| (i64t, self.loc)).collect();
        let func_type = FunctionType::new(self.ctx, &vec![i64t; n_mlir_params], &[i64t]);

        let region = Region::new();
        let entry = region.append_block(Block::new(&param_specs));

        // Collect arg values (skip env at index 0).
        let mut runtime_args: Vec<Value<'_, '_>> = Vec::new();
        for i in 1..n_mlir_params {
            runtime_args.push(entry.argument(i)?.into());
        }

        let result: Value<'_, '_> = entry.append_operation(func::call(
            self.ctx,
            FlatSymbolRefAttribute::new(self.ctx, runtime_fn),
            &runtime_args,
            &[i64t],
            self.loc,
        )).result(0)?.into();
        entry.append_operation(func::r#return(&[result], self.loc));

        let fn_op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, wrapper_name),
            TypeAttribute::new(func_type.into()),
            region,
            &[],
            self.loc,
        );
        self.module.body().append_operation(fn_op);
        Ok(Some(wrapper_name.to_string()))
    }

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
        // For NaN-boxed i64 values, use ts_is_truthy so that NaN-boxed false/null/undefined
        // are treated as falsy. Direct != 0 comparison is wrong because FALSE is non-zero.
        let cmp_val = if val.r#type() == self.i64_type() {
            block.append_operation(func::call(
                self.ctx,
                melior::ir::attribute::FlatSymbolRefAttribute::new(self.ctx, "ts_is_truthy"),
                &[val],
                &[self.i32_type()],
                self.loc,
            )).result(0)?.into()
        } else {
            val
        };
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
                cmp_val,
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
            // Integer: NAN_MASK | TAG_INT | (u32 value)
            let extended = block.append_operation(arith::extui(val, self.i64_type(), self.loc)).result(0)?.into();
            let mask = block.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i64_type(), 0x7FFE_0000_0000_0000u64 as i64).into(),
                self.loc,
            )).result(0)?.into();
            Ok(block.append_operation(arith::ori(extended, mask, self.loc)).result(0)?.into())
        } else if ty == self.i1_type() {
            // Boolean: NAN_MASK | TAG_BOOL | (0 or 1)
            // NAN_MASK=0x7FF8… TAG_BOOL=0x0002… → combined=0x7FFA_0000_0000_0000
            let extended = block.append_operation(arith::extui(val, self.i64_type(), self.loc)).result(0)?.into();
            let bool_mask = block.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(self.i64_type(), 0x7FFA_0000_0000_0000u64 as i64).into(),
                self.loc,
            )).result(0)?.into();
            Ok(block.append_operation(arith::ori(extended, bool_mask, self.loc)).result(0)?.into())
        } else {
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
            let coerced = if self.fn_return_type == self.i64_type() {
                self.ensure_i64(val, block)?
            } else {
                self.ensure_i32(val, block)?
            };
            block.append_operation(func::r#return(&[coerced], self.loc));
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

    /// Coerce `val` to `expected` type using existing ensure_* helpers.
    /// Used to normalise scope values before passing them as phi-block arguments.
    pub(super) fn coerce_val_to_type<'b>(
        &self,
        val: Value<'c, 'b>,
        expected: melior::ir::Type<'c>,
        block: BlockRef<'c, 'b>,
    ) -> Result<Value<'c, 'b>> {
        if val.r#type() == expected {
            return Ok(val);
        }
        if expected == self.i64_type() {
            self.ensure_i64(val, block)
        } else if expected == self.i32_type() {
            self.ensure_i32(val, block)
        } else if expected == self.i1_type() {
            self.ensure_i1(val, block)
        } else {
            Ok(val)
        }
    }

    // ── Runtime external declarations ─────────────────────────────────────

    pub(super) fn emit_runtime_declarations(&mut self) {
        let i32_type  = self.i32_type();
        let i64_type = self.i64_type();
        let ptr_type  = self.llvm_ptr_type();
        let private   = &[(
            Identifier::new(self.ctx, "sym_visibility"),
            StringAttribute::new(self.ctx, "private").into(),
        )];

        let add_func = |name: &str, params: &[Type<'c>], results: &[Type<'c>]| {
            let op = func::func(
                self.ctx,
                StringAttribute::new(self.ctx, name),
                TypeAttribute::new(FunctionType::new(self.ctx, params, results).into()),
                Region::new(),
                private,
                self.loc,
            );
            self.module.body().append_operation(op);
        };

        add_func("__ts_console_log_i32", &[i32_type], &[]);
        add_func("__ts_console_log_val", &[i64_type], &[]);
        add_func("ts_retain_val", &[i64_type], &[]);
        add_func("ts_retain", &[ptr_type], &[]);
        add_func("ts_release", &[ptr_type, ptr_type], &[]);
        add_func("ts_release_val", &[i64_type], &[]);
        
        add_func("ts_obj_new", &[], &[i64_type]);
        add_func("ts_obj_get", &[i64_type, ptr_type], &[i64_type]);
        add_func("ts_obj_set", &[i64_type, ptr_type, i64_type], &[]);
        
        add_func("ts_arr_new", &[i32_type], &[i64_type]);
        add_func("ts_arr_get", &[i64_type, i32_type], &[i64_type]);
        add_func("ts_arr_set", &[i64_type, i32_type, i64_type], &[]);
        add_func("ts_arr_len", &[i64_type], &[i64_type]);

        add_func("ts_string_new", &[ptr_type], &[i64_type]);
        add_func("ts_string_concat", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_add", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_sub", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_mul", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_div", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_mod", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_lt",  &[i64_type, i64_type], &[i32_type]);
        add_func("ts_le",  &[i64_type, i64_type], &[i32_type]);
        add_func("ts_gt",  &[i64_type, i64_type], &[i32_type]);
        add_func("ts_ge",  &[i64_type, i64_type], &[i32_type]);

        add_func("ts_promise_resolve", &[i64_type], &[i64_type]);
        add_func("ts_promise_await",   &[i64_type], &[i64_type]);

        add_func("ts_throw",            &[i64_type], &[]);
        add_func("ts_check_exception",  &[], &[i32_type]);
        add_func("ts_catch_exception",  &[], &[i64_type]);

        add_func("ts_sleep",           &[i32_type], &[i64_type]);
        add_func("ts_promise_race",    &[i64_type, i64_type], &[i64_type]);

        add_func("ts_async_spawn0",    &[ptr_type], &[i64_type]);
        add_func("ts_async_spawn1",    &[ptr_type, i32_type], &[i64_type]);
        add_func("ts_async_spawn2",    &[ptr_type, i32_type, i32_type], &[i64_type]);
        add_func("ts_async_spawn3",    &[ptr_type, i32_type, i32_type, i32_type], &[i64_type]);
        add_func("ts_async_spawn4",    &[ptr_type, i32_type, i32_type, i32_type, i32_type], &[i64_type]);

        add_func("ts_typeof",          &[i64_type], &[i64_type]);
        add_func("ts_val_strict_eq",   &[i64_type, i64_type], &[i32_type]);
        add_func("ts_is_nullish",      &[i64_type], &[i32_type]);
        add_func("ts_is_truthy",       &[i64_type], &[i32_type]);
        add_func("ts_is_undefined",    &[i64_type], &[i32_type]);
        add_func("ts_obj_set_val_key", &[i64_type, i64_type, i64_type], &[]);
        add_func("ts_obj_keys",        &[i64_type], &[i64_type]);

        // v1.0: template literals, array/string methods, spread
        add_func("ts_val_to_string",   &[i64_type], &[i64_type]);
        add_func("ts_val_length",      &[i64_type], &[i64_type]);
        add_func("ts_arr_push",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_pop",         &[i64_type], &[i64_type]);
        add_func("ts_arr_push_all",    &[i64_type, i64_type], &[]);
        add_func("ts_arr_join",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_index_of",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_val_index_of",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_val_includes",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_index_of",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_includes",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_slice",       &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_str_to_upper",    &[i64_type], &[i64_type]);
        add_func("ts_str_to_lower",    &[i64_type], &[i64_type]);
        add_func("ts_str_trim",        &[i64_type], &[i64_type]);
        add_func("ts_str_split",       &[i64_type, i64_type], &[i64_type]);

        // v1.1: Math, Object statics, Array.isArray, parseInt/parseFloat, console.log multi-arg
        add_func("ts_math_abs",   &[i64_type], &[i64_type]);
        add_func("ts_math_floor", &[i64_type], &[i64_type]);
        add_func("ts_math_ceil",  &[i64_type], &[i64_type]);
        add_func("ts_math_round", &[i64_type], &[i64_type]);
        add_func("ts_math_sqrt",  &[i64_type], &[i64_type]);
        add_func("ts_math_trunc", &[i64_type], &[i64_type]);
        add_func("ts_math_log",   &[i64_type], &[i64_type]);
        add_func("ts_math_log2",  &[i64_type], &[i64_type]);
        add_func("ts_math_log10", &[i64_type], &[i64_type]);
        add_func("ts_math_sin",   &[i64_type], &[i64_type]);
        add_func("ts_math_cos",   &[i64_type], &[i64_type]);
        add_func("ts_math_tan",   &[i64_type], &[i64_type]);
        add_func("ts_math_min",   &[i64_type, i64_type], &[i64_type]);
        add_func("ts_math_max",   &[i64_type, i64_type], &[i64_type]);
        add_func("ts_math_pow",   &[i64_type, i64_type], &[i64_type]);
        add_func("ts_math_atan2", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_math_hypot", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_obj_values",  &[i64_type], &[i64_type]);
        add_func("ts_obj_entries", &[i64_type], &[i64_type]);
        add_func("ts_obj_merge",          &[i64_type, i64_type], &[]);
        add_func("ts_obj_assign",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_obj_create",         &[i64_type], &[i64_type]);
        add_func("ts_obj_from_entries",   &[i64_type], &[i64_type]);
        add_func("ts_is_array",    &[i64_type], &[i32_type]);
        add_func("ts_parse_int",   &[i64_type, i64_type], &[i64_type]);
        add_func("ts_parse_float", &[i64_type], &[i64_type]);
        add_func("ts_is_nan_val",  &[i64_type], &[i64_type]);
        add_func("ts_is_finite_val", &[i64_type], &[i64_type]);
        add_func("__ts_console_log_val_inline", &[i64_type], &[]);
        add_func("__ts_console_log_space",      &[], &[]);
        add_func("__ts_console_log_newline",    &[], &[]);

        // v1.2: first-class functions / arrow functions / array HOFs
        add_func("ts_func_new",        &[ptr_type, i32_type], &[i64_type]);
        add_func("ts_closure_new",     &[ptr_type, i32_type, i64_type], &[i64_type]);
        add_func("ts_func_call0",      &[i64_type], &[i64_type]);
        add_func("ts_func_call1",      &[i64_type, i64_type], &[i64_type]);
        add_func("ts_func_call2",      &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_func_call3",      &[i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_func_call4",      &[i64_type, i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_map",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_filter",      &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_for_each",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_reduce",      &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_find",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_find_index",  &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_some",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_every",       &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_sort",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_flat_map",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_flat",        &[i64_type, i32_type], &[i64_type]);

        // v1.4: Map built-in
        add_func("ts_map_new",      &[], &[i64_type]);
        add_func("ts_map_set",      &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_map_get",      &[i64_type, i64_type], &[i64_type]);
        add_func("ts_map_has",      &[i64_type, i64_type], &[i64_type]);
        add_func("ts_map_delete",   &[i64_type, i64_type], &[i64_type]);
        add_func("ts_map_clear",    &[i64_type], &[]);
        add_func("ts_map_size",     &[i64_type], &[i64_type]);
        add_func("ts_map_keys",     &[i64_type], &[i64_type]);
        add_func("ts_map_values",   &[i64_type], &[i64_type]);
        add_func("ts_map_for_each", &[i64_type, i64_type], &[i64_type]);

        // v1.3: additional string methods
        add_func("ts_str_replace",        &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_str_replace_all",    &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_str_starts_with",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_ends_with",      &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_pad_start",      &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_str_pad_end",        &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_str_char_at",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_char_code_at",   &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_repeat",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_from_char_code", &[i64_type], &[i64_type]);

        // v1.5: generic computed member get, destructuring rest, Map.entries
        add_func("ts_val_get_key", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_rest",    &[i64_type, i32_type], &[i64_type]);
        add_func("ts_obj_rest",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_map_entries", &[i64_type], &[i64_type]);

        add_func("ts_json_stringify",  &[i64_type], &[i64_type]);
        add_func("ts_json_parse",      &[i64_type], &[i64_type]);
        add_func("ts_coerce_number",   &[i64_type], &[i64_type]);
        add_func("ts_coerce_string",   &[i64_type], &[i64_type]);
        add_func("ts_func_spread_call",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_encode_uri_component",&[i64_type], &[i64_type]);
        add_func("ts_decode_uri_component",&[i64_type], &[i64_type]);
        add_func("ts_encode_uri",          &[i64_type], &[i64_type]);
        add_func("ts_decode_uri",          &[i64_type], &[i64_type]);

        add_func("ts_regexp_new",          &[ptr_type, ptr_type], &[i64_type]);
        add_func("ts_regexp_from_val",     &[i64_type, i64_type], &[i64_type]);
        add_func("ts_regexp_test",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_regexp_exec",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_match",           &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_replace_regex",   &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_regexp_source",       &[i64_type], &[i64_type]);

        // v1.6: Error built-in
        add_func("ts_error_new",           &[i64_type], &[i64_type]);

        // Object delete
        add_func("ts_obj_delete",          &[i64_type, ptr_type], &[i64_type]);
        add_func("ts_obj_delete_key",      &[i64_type, i64_type], &[i64_type]);

        // Logical NOT of truthy result
        add_func("ts_val_not",             &[i32_type], &[i32_type]);

        // Web/Fetch API
        add_func("ts_headers_new",         &[i64_type], &[i64_type]);
        add_func("ts_headers_append",      &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_headers_get_set_cookie", &[i64_type], &[i64_type]);
        add_func("ts_response_new",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_response_clone",      &[i64_type], &[i64_type]);
        add_func("ts_request_new",         &[i64_type, i64_type], &[i64_type]);

        // Module globals (cross-function shared state)
        add_func("ts_set_module_global",   &[ptr_type, i64_type], &[]);
        add_func("ts_get_module_global",   &[ptr_type], &[i64_type]);

        // Additional builtins
        add_func("ts_promise_reject",      &[i64_type], &[i64_type]);
        add_func("ts_promise_all",         &[i64_type], &[i64_type]);
        add_func("ts_val_has_key",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_coerce_bool",         &[i64_type], &[i64_type]);
    }

    /// Returns true if `class` is `target` or transitively inherits from `target`.
    pub(super) fn is_subclass_of(&self, class: &str, target: &str) -> bool {
        if class == target { return true; }
        if let Some(sig) = self.classes.get(class) {
            if let Some(parent) = &sig.parent {
                return self.is_subclass_of(parent, target);
            }
        }
        false
    }


    // ── Function signature collection (hoisting pass) ─────────────────────

    pub(super) fn collect_function_signatures(&mut self, program: &Program<'_>) {
        let i64_type = self.i64_type();
        for stmt in &program.body {
            match stmt {
                Statement::FunctionDeclaration(func) => {
                    let Some(id) = &func.id else { continue };
                    let name = id.name.to_string();
                    let has_rest = func.params.rest.is_some();
                    let n = func.params.items.len() + if has_rest { 1 } else { 0 };
                    self.funcs.insert(name, FuncSig {
                        param_types: vec![i64_type; n],
                        return_type: Some(i64_type),
                        has_rest,
                    });
                }
                Statement::ExportNamedDeclaration(export) => {
                    if let Some(Declaration::FunctionDeclaration(func)) = &export.declaration {
                        if let Some(id) = &func.id {
                            let name = id.name.to_string();
                            let has_rest = func.params.rest.is_some();
                            let n = func.params.items.len() + if has_rest { 1 } else { 0 };
                            self.funcs.insert(name, FuncSig {
                                param_types: vec![i64_type; n],
                                return_type: Some(i64_type),
                                has_rest,
                            });
                        }
                    }
                    if let Some(Declaration::VariableDeclaration(vd)) = &export.declaration {
                        self.collect_const_sigs(vd, i64_type);
                    }
                }
                Statement::ExportDefaultDeclaration(export) => {
                    use oxc_ast::ast::ExportDefaultDeclarationKind;
                    if let ExportDefaultDeclarationKind::FunctionDeclaration(func) = &export.declaration {
                        if let Some(id) = &func.id {
                            let name = id.name.to_string();
                            let has_rest = func.params.rest.is_some();
                            let n = func.params.items.len() + if has_rest { 1 } else { 0 };
                            self.funcs.insert(name, FuncSig {
                                param_types: vec![i64_type; n],
                                return_type: Some(i64_type),
                                has_rest,
                            });
                        }
                    }
                }
                // Module-level `const name = arrow` — hoist as function.
                // Module-level `const name = identifier` — track as alias.
                Statement::VariableDeclaration(vd) => {
                    self.collect_const_sigs(vd, i64_type);
                }
                _ => {}
            }
        }
    }

    /// Helper: scan a `VariableDeclaration` for module-level `const name = arrow/fn/ident`.
    fn collect_const_sigs(&mut self, vd: &oxc_ast::ast::VariableDeclaration<'_>, i64_type: melior::ir::Type<'c>) {
        for decl in &vd.declarations {
            let name = match &decl.id {
                BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                _ => continue,
            };
            let init = match &decl.init {
                Some(e) => e,
                None => continue,
            };
            // Strip TS type casts to get to the underlying expression.
            let inner = Self::strip_ts_casts(init);
            match inner {
                Expression::ArrowFunctionExpression(arrow) => {
                    let has_rest = arrow.params.rest.is_some();
                    let n = arrow.params.items.len() + if has_rest { 1 } else { 0 };
                    self.funcs.insert(name, FuncSig {
                        param_types: vec![i64_type; n],
                        return_type: Some(i64_type),
                        has_rest,
                    });
                }
                Expression::FunctionExpression(func_expr) => {
                    let has_rest = func_expr.params.rest.is_some();
                    let n = func_expr.params.items.len() + if has_rest { 1 } else { 0 };
                    self.funcs.insert(name, FuncSig {
                        param_types: vec![i64_type; n],
                        return_type: Some(i64_type),
                        has_rest,
                    });
                }
                Expression::Identifier(id) => {
                    // `const alias = someFunc` — record as alias for call dispatch.
                    self.builtin_aliases.insert(name, id.name.to_string());
                }
                _ => {
                    // Non-function const (object, array, literal, etc.) — it's a module global.
                    self.module_global_names.insert(name);
                }
            }
        }
    }

    /// Strip TS type assertions/casts recursively (as, satisfies, non-null, assertion).
    fn strip_ts_casts<'e>(expr: &'e Expression<'_>) -> &'e Expression<'e> {
        match expr {
            Expression::TSAsExpression(e) => Self::strip_ts_casts(&e.expression),
            Expression::TSSatisfiesExpression(e) => Self::strip_ts_casts(&e.expression),
            Expression::TSNonNullExpression(e) => Self::strip_ts_casts(&e.expression),
            Expression::TSTypeAssertion(e) => Self::strip_ts_casts(&e.expression),
            Expression::ParenthesizedExpression(e) => Self::strip_ts_casts(&e.expression),
            other => other,
        }
    }

    pub(super) fn collect_class_definitions(&mut self, program: &Program<'_>) {
        // Pass 1: collect own members for each class
        let mut own_members: Vec<(String, ClassSig)> = Vec::new();

        for stmt in &program.body {
            let class_opt: Option<&Class<'_>> = match stmt {
                Statement::ClassDeclaration(class) => Some(class),
                Statement::ExportNamedDeclaration(export) => {
                    if let Some(Declaration::ClassDeclaration(class)) = &export.declaration {
                        Some(class)
                    } else {
                        None
                    }
                }
                Statement::ExportDefaultDeclaration(export) => {
                    use oxc_ast::ast::ExportDefaultDeclarationKind;
                    if let ExportDefaultDeclarationKind::ClassDeclaration(class) = &export.declaration {
                        Some(class)
                    } else {
                        None
                    }
                }
                _ => None,
            };

            let Some(class) = class_opt else { continue };
            let Some(id) = &class.id else { continue };
            let class_name = id.name.to_string();

            let mut methods: HashMap<String, String> = HashMap::new();
            let mut statics: HashMap<String, String> = HashMap::new();
            let mut getters: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut setters: std::collections::HashSet<String> = std::collections::HashSet::new();

            for element in &class.body.body {
                let ClassElement::MethodDefinition(method) = element else { continue };
                if method.kind == MethodDefinitionKind::Constructor { continue; }
                // Skip overload signatures (no body).
                if method.value.body.is_none() { continue; }

                // Resolve method name: public (StaticIdentifier) or private (#name)
                let name_opt: Option<String> = match &method.key {
                    oxc_ast::ast::PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
                    oxc_ast::ast::PropertyKey::PrivateIdentifier(id) => {
                        Some(format!("__priv_{}", id.name.as_str()))
                    }
                    _ => method.key.static_name().map(|n| n.to_string()),
                };
                let Some(name) = name_opt else { continue };

                match (method.kind, method.r#static) {
                    (MethodDefinitionKind::Get, false) => {
                        getters.insert(name);
                    }
                    (MethodDefinitionKind::Set, false) => {
                        setters.insert(name);
                    }
                    (MethodDefinitionKind::Method, true) => {
                        let mangled = format!("__class_{}_static_{}", class_name, name);
                        statics.insert(name, mangled);
                    }
                    (MethodDefinitionKind::Method, false) => {
                        let mangled = format!("__class_{}_{}", class_name, name);
                        methods.insert(name, mangled);
                    }
                    _ => {}
                }
            }

            let parent = class.super_class.as_ref().and_then(|e| {
                if let Expression::Identifier(id) = e { Some(id.name.to_string()) } else { None }
            });

            own_members.push((class_name.clone(), ClassSig {
                constructor_name: format!("__class_{}_constructor", class_name),
                methods,
                statics,
                getters,
                setters,
                parent,
            }));
        }

        // Pass 2: insert in order; inherit from parent (already inserted if declared first)
        for (class_name, mut sig) in own_members {
            if let Some(parent_name) = sig.parent.clone() {
                if let Some(parent_sig) = self.classes.get(&parent_name).cloned() {
                    for (n, m) in &parent_sig.methods {
                        sig.methods.entry(n.clone()).or_insert_with(|| m.clone());
                    }
                    for n in &parent_sig.getters {
                        if !sig.getters.contains(n) { sig.getters.insert(n.clone()); }
                    }
                    for n in &parent_sig.setters {
                        if !sig.setters.contains(n) { sig.setters.insert(n.clone()); }
                    }
                }
            }
            self.classes.insert(class_name, sig);
        }
    }

    // ── Function declarations ─────────────────────────────────────────────

    pub fn lower_function_declaration(&mut self, func: &Function<'_>) -> Result<()> {
        let Some(id) = &func.id else { return Ok(()) };
        let name = id.name.to_string();
        let i32_type = self.i32_type();
        let i64_type = self.i64_type();
        // All functions return i64 (NaN-boxed TsVal) so they can return any value including heap objects.
        let return_type = i64_type;

        let has_rest = func.params.rest.is_some();
        // Use i64 for all params to support NaN-boxed values (including `undefined` for defaults).
        let n_params = func.params.items.len() + if has_rest { 1 } else { 0 };
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..n_params).map(|_| (i64_type, self.loc)).collect();
        let func_type = FunctionType::new(
            self.ctx, &vec![i64_type; n_params], &[return_type],
        );

        let region = Region::new();
        let entry  = region.append_block(Block::new(&param_specs));

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        for (i, param) in func.params.items.iter().enumerate() {
            if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                scope.insert(id.name.to_string(), entry.argument(i)?.into());
            }
        }
        // Bind the rest parameter (last MLIR param) as a TsArray in scope.
        if let Some(rest_param) = &func.params.rest {
            if let BindingPattern::BindingIdentifier(rest_id) = &rest_param.rest.argument {
                let rest_arg_idx = func.params.items.len();
                let rest_val: Value<'_, '_> = entry.argument(rest_arg_idx)?.into();
                scope.insert(rest_id.name.to_string(), rest_val);
            }
        }

        let mut current_block = entry;

        // Emit default parameter checks: if param === undefined, use initializer.
        for (i, param) in func.params.items.iter().enumerate() {
            let Some(init_expr) = &param.initializer else { continue };
            let BindingPattern::BindingIdentifier(id) = &param.pattern else { continue };
            let param_name = id.name.to_string();

            let param_val: Value<'_, '_> = entry.argument(i)?.into();
            let param_i64 = self.ensure_i64(param_val, current_block)?;

            let is_undef: Value<'_, '_> = current_block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_undefined"),
                &[param_i64], &[i32_type], self.loc,
            )).result(0)?.into();
            let is_undef_i1 = self.ensure_i1(is_undef, current_block)?;

            let merge_block = region.append_block(Block::new(&[(i64_type, self.loc)]));
            let default_block = region.append_block(Block::new(&[]));

            current_block.append_operation(cf::cond_br(
                self.ctx, is_undef_i1,
                &default_block, &merge_block,
                &[], &[param_i64],
                self.loc,
            ));

            let mut default_scope = scope.clone();
            let (init_val_opt, post_init_block) =
                self.lower_expression(init_expr, default_block, &region, &mut default_scope)?;
            let init_val = init_val_opt.ok_or_else(|| anyhow::anyhow!("default param '{}': initializer produced no value", param_name))?;
            let init_i64 = self.ensure_i64(init_val, post_init_block)?;
            post_init_block.append_operation(cf::br(&merge_block, &[init_i64], self.loc));

            let final_param: Value<'_, '_> = merge_block.argument(0)?.into();
            scope.insert(param_name, final_param);
            current_block = merge_block;
        }
        let mut result_value: Value<'_, '_> = entry
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(return_type, 0).into(),
                self.loc,
            ))
            .result(0)?.into();

        self.fn_return_type = return_type;
        self.is_async = func.r#async;
        if let Some(body) = &func.body {
            for stmt in &body.statements {
                let (val, next) = self.lower_statement(stmt, current_block, &region, &mut scope, &[])?;
                current_block = next;
                if let Some(v) = val { result_value = v; }
            }
        }
        self.is_async = false;
        self.fn_return_type = i32_type;

        // ARC: release scope variables before final return.
        for (_, v) in &scope {
            let v_i64 = self.ensure_i64(*v, current_block)?;
            current_block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[v_i64], &[], self.loc,
            ));
        }

        // Async: wrap the implicit return value in a resolved Promise.
        if current_block.terminator().is_none() && func.r#async {
            let val_i64 = self.ensure_i64(result_value, current_block)?;
            let promise: Value<'_, '_> = current_block
                .append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                    &[val_i64], &[i64_type], self.loc,
                ))
                .result(0)?.into();
            current_block.append_operation(func::r#return(&[promise], self.loc));
        } else {
            self.terminate_with_return(current_block, result_value)?;
        }

        let op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &name),
            TypeAttribute::new(func_type.into()),
            region,
            &[],
            self.loc,
        );
        self.module.body().append_operation(op);

        self.funcs.insert(name, FuncSig {
            param_types: vec![i64_type; param_specs.len()],
            return_type: Some(return_type),
            has_rest,
        });
        Ok(())
    }

    /// Lower a function by name (for module-level const arrow hoisting).
    /// Similar to `lower_function_declaration` but takes params/body directly.
    pub(super) fn lower_named_function(
        &mut self,
        name: &str,
        params: &[&oxc_ast::ast::FormalParameter<'_>],
        rest_param_name: Option<&str>,
        body: Option<&oxc_ast::ast::FunctionBody<'_>>,
        is_async_override: Option<bool>,
    ) -> Result<()> {
        let i64_type = self.i64_type();
        let i32_type = self.i32_type();
        let return_type = i64_type;

        let has_rest = rest_param_name.is_some();
        let n_params = params.len() + if has_rest { 1 } else { 0 };
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..n_params).map(|_| (i64_type, self.loc)).collect();
        let func_type = FunctionType::new(self.ctx, &vec![i64_type; n_params], &[return_type]);

        let region = Region::new();
        let entry = region.append_block(Block::new(&param_specs));

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        for (i, param) in params.iter().enumerate() {
            let arg_val: Value<'_, '_> = entry.argument(i)?.into();
            match &param.pattern {
                BindingPattern::BindingIdentifier(id) => {
                    scope.insert(id.name.to_string(), arg_val);
                }
                BindingPattern::ArrayPattern(arr_pat) => {
                    let arg_i64 = self.ensure_i64(arg_val, entry)?;
                    for (elem_idx, elem) in arr_pat.elements.iter().enumerate() {
                        if let Some(BindingPattern::BindingIdentifier(id)) = elem {
                            let idx_c: Value<'_, '_> = entry.append_operation(arith::constant(
                                self.ctx, IntegerAttribute::new(self.i32_type(), elem_idx as i64).into(), self.loc,
                            )).result(0)?.into();
                            let elem_val: Value<'_, '_> = entry.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
                                &[arg_i64, idx_c], &[i64_type], self.loc,
                            )).result(0)?.into();
                            scope.insert(id.name.to_string(), elem_val);
                        }
                    }
                }
                BindingPattern::ObjectPattern(obj_pat) => {
                    let arg_i64 = self.ensure_i64(arg_val, entry)?;
                    for prop in &obj_pat.properties {
                        if let (oxc_ast::ast::PropertyKey::StaticIdentifier(key_id),
                                BindingPattern::BindingIdentifier(val_id)) =
                            (&prop.key, &prop.value)
                        {
                            let key_ptr = self.get_string_ptr(key_id.name.as_str(), entry)?;
                            let prop_val: Value<'_, '_> = entry.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_obj_get"),
                                &[arg_i64, key_ptr], &[i64_type], self.loc,
                            )).result(0)?.into();
                            scope.insert(val_id.name.to_string(), prop_val);
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some(rest_name) = rest_param_name {
            let rest_idx = params.len();
            scope.insert(rest_name.to_string(), entry.argument(rest_idx)?.into());
        }

        let mut current_block = entry;

        // Inject module-level global variables into scope via ts_get_module_global.
        // This allows module-level functions to access module-level non-function consts.
        for global_name in self.module_global_names.clone() {
            if !scope.contains_key(&global_name) {
                let key_ptr = self.get_string_ptr(&global_name, entry)?;
                let val: Value<'_, '_> = entry.append_operation(func::call(
                    self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_get_module_global"),
                    &[key_ptr], &[i64_type], self.loc,
                )).result(0)?.into();
                scope.insert(global_name, val);
            }
        }

        // Handle default parameters.
        for (i, param) in params.iter().enumerate() {
            let Some(init_expr) = &param.initializer else { continue };
            let BindingPattern::BindingIdentifier(id) = &param.pattern else { continue };
            let param_name = id.name.to_string();
            let param_val: Value<'_, '_> = entry.argument(i)?.into();
            let param_i64 = self.ensure_i64(param_val, current_block)?;
            let is_undef: Value<'_, '_> = current_block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_is_undefined"),
                &[param_i64], &[i32_type], self.loc,
            )).result(0)?.into();
            let is_undef_i1 = self.ensure_i1(is_undef, current_block)?;
            let merge_block = region.append_block(Block::new(&[(i64_type, self.loc)]));
            let default_block = region.append_block(Block::new(&[]));
            current_block.append_operation(cf::cond_br(
                self.ctx, is_undef_i1, &default_block, &merge_block, &[], &[param_i64], self.loc,
            ));
            let mut default_scope = scope.clone();
            let (init_val_opt, post_init_block) =
                self.lower_expression(init_expr, default_block, &region, &mut default_scope)?;
            let init_val = init_val_opt.ok_or_else(|| anyhow::anyhow!("default param: no value"))?;
            let init_i64 = self.ensure_i64(init_val, post_init_block)?;
            post_init_block.append_operation(cf::br(&merge_block, &[init_i64], self.loc));
            let final_param: Value<'_, '_> = merge_block.argument(0)?.into();
            scope.insert(param_name, final_param);
            current_block = merge_block;
        }

        let mut result_value: Value<'_, '_> = entry.append_operation(arith::constant(
            self.ctx, IntegerAttribute::new(return_type, 0).into(), self.loc,
        )).result(0)?.into();

        let is_async = is_async_override.unwrap_or(false);
        self.fn_return_type = return_type;
        self.is_async = is_async;
        if let Some(body) = body {
            for stmt in &body.statements {
                let (val, next) = self.lower_statement(stmt, current_block, &region, &mut scope, &[])?;
                current_block = next;
                if let Some(v) = val { result_value = v; }
            }
        }
        self.is_async = false;

        for (_, v) in &scope {
            let v_i64 = self.ensure_i64(*v, current_block)?;
            current_block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[v_i64], &[], self.loc,
            ));
        }

        if current_block.terminator().is_none() && is_async {
            let val_i64 = self.ensure_i64(result_value, current_block)?;
            let promise: Value<'_, '_> = current_block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                &[val_i64], &[i64_type], self.loc,
            )).result(0)?.into();
            current_block.append_operation(func::r#return(&[promise], self.loc));
        } else {
            self.terminate_with_return(current_block, result_value)?;
        }
        self.fn_return_type = i32_type;

        let op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, name),
            TypeAttribute::new(func_type.into()),
            region,
            &[],
            self.loc,
        );
        self.module.body().append_operation(op);
        Ok(())
    }
}
