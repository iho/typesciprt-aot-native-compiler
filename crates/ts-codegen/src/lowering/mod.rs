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
use melior::ir::operation::{OperationBuilder, OperationLike};
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
mod call;
mod optional;
mod closures;
mod free_vars;
mod analysis;
mod literals;
mod operators;
mod classes;
mod enums;

pub(crate) use free_vars::{NameSet, collect_free_vars_stmts, compute_cell_vars_for_body, body_uses_arguments, collect_locals_binding, predeclare_binding};
pub(crate) use analysis::{compute_scalar_vars_for_body, compute_non_escaping_allocs};

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
    /// True if the function has an explicit TypeScript `this` parameter.
    /// In this case the first MLIR param is the `this` value.
    has_this_param: bool,
}
 
#[derive(Clone)]
struct ClassSig {
    constructor_name: String,
    /// Number of MLIR params the constructor takes.
    constructor_arity: usize,
    methods:  HashMap<String, String>,                  // instance method name → mangled
    /// Arity of each instance method (number of MLIR params including `self`).
    method_arity: HashMap<String, usize>,
    /// Instance methods that have a rest parameter (last MLIR param is a TsArray).
    method_has_rest: std::collections::HashSet<String>,
    statics:  HashMap<String, String>,                  // static method name → mangled
    getters:  std::collections::HashSet<String>,        // property names with getters
    setters:  std::collections::HashSet<String>,        // property names with setters
    static_fields: std::collections::HashSet<String>,  // static property names (stored as module globals)
    /// Fields that hold user-class instances: field_name → class_name.
    /// Populated from constructor parameter properties with class type annotations.
    /// Used to avoid dispatching HOF methods (find/filter/etc.) to array builtins
    /// when the receiver is `this.field` and the field holds a user-class instance.
    field_class_types: HashMap<String, String>,
    parent:   Option<String>,
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Parse a local import file and return its Program with 'static lifetime.
/// Uses Box::leak so the allocator lives for the process lifetime (acceptable for a compiler).
/// Returns the parsed program and whether the source was detected as CommonJS.
fn load_import_static(path: &std::path::Path) -> Option<(oxc_ast::ast::Program<'static>, bool)> {
    let source_str = std::fs::read_to_string(path).ok()?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let source: &'static str = Box::leak(source_str.into_boxed_str());
    let alloc: &'static oxc_allocator::Allocator =
        Box::leak(Box::new(oxc_allocator::Allocator::default()));
    let file_name = path.display().to_string();

    match ext {
        "ts" | "tsx" | "d.ts" => {
            let prog = ts_frontend::parse_source(alloc, source, &file_name, oxc_span::SourceType::ts()).ok()?;
            Some((prog, false))
        }
        "mjs" => {
            let prog = ts_frontend::parse_source(alloc, source, &file_name, oxc_span::SourceType::mjs()).ok()?;
            Some((prog, false))
        }
        "cjs" => {
            let prog = ts_frontend::parse_source(alloc, source, &file_name, oxc_span::SourceType::cjs()).ok()?;
            Some((prog, true))
        }
        "js" => {
            // Try ESM first, then CJS, then unambiguous
            if let Ok(prog) = ts_frontend::parse_source(alloc, source, &file_name, oxc_span::SourceType::mjs()) {
                // Detect CJS by looking for require() calls or module.exports
                let is_cjs = is_cjs_program(&prog);
                Some((prog, is_cjs))
            } else if let Ok(prog) = ts_frontend::parse_source(alloc, source, &file_name, oxc_span::SourceType::cjs()) {
                Some((prog, true))
            } else {
                let prog = ts_frontend::parse_source(alloc, source, &file_name, oxc_span::SourceType::unambiguous()).ok()?;
                let is_cjs = is_cjs_program(&prog);
                Some((prog, is_cjs))
            }
        }
        _ => {
            // Default: try as TypeScript
            let prog = ts_frontend::parse_source(alloc, source, &file_name, oxc_span::SourceType::ts()).ok()?;
            Some((prog, false))
        }
    }
}

/// Detect whether a parsed program is CommonJS by checking for require() calls
/// or module.exports / exports.* assignments at the top level or within functions.
fn is_cjs_program(program: &oxc_ast::ast::Program<'_>) -> bool {
    use oxc_ast::ast::{Statement, Expression, MemberExpression};
    for stmt in &program.body {
        if stmt_has_cjs_markers(stmt) {
            return true;
        }
    }
    false
}

fn stmt_has_cjs_markers(stmt: &oxc_ast::ast::Statement<'_>) -> bool {
    use oxc_ast::ast::{Statement, Expression};
    match stmt {
        Statement::ExpressionStatement(expr_stmt) => {
            expr_has_cjs_markers(&expr_stmt.expression)
        }
        Statement::VariableDeclaration(vd) => {
            vd.declarations.iter().any(|decl| {
                decl.init.as_ref().map_or(false, |init| expr_has_cjs_markers(init))
            })
        }
        Statement::BlockStatement(block) => {
            block.body.iter().any(stmt_has_cjs_markers)
        }
        Statement::IfStatement(if_stmt) => {
            stmt_has_cjs_markers(&if_stmt.consequent) ||
            if_stmt.alternate.as_ref().map_or(false, |alt| stmt_has_cjs_markers(alt))
        }
        _ => false,
    }
}

fn expr_has_cjs_markers(expr: &oxc_ast::ast::Expression<'_>) -> bool {
    use oxc_ast::ast::{Expression, AssignmentTarget, SimpleAssignmentTarget, MemberExpression};
    match expr {
        // require('...') call
        Expression::CallExpression(call) => {
            if let Expression::Identifier(id) = &call.callee {
                if id.name == "require" {
                    return true;
                }
            }
            // Recurse into arguments
            call.arguments.iter().any(|arg| {
                arg.as_expression().map_or(false, expr_has_cjs_markers)
            })
        }
        // module.exports = ... or exports.foo = ...
        Expression::AssignmentExpression(assign) => {
            let lhs_is_cjs = match &assign.left {
                AssignmentTarget::StaticMemberExpression(member) => {
                    let obj_is_module = if let Expression::Identifier(id) = &member.object {
                        id.name == "module" || id.name == "exports"
                    } else { false };
                    let obj_is_module_exports = if let Expression::StaticMemberExpression(inner) = &member.object {
                        if let Expression::Identifier(id) = &inner.object {
                            id.name == "module" && inner.property.name == "exports"
                        } else { false }
                    } else { false };
                    obj_is_module || obj_is_module_exports
                }
                _ => false,
            };
            lhs_is_cjs || expr_has_cjs_markers(&assign.right)
        }
        _ => false,
    }
}

/// Resolve a relative import source string to an actual `.ts` file path.
/// Tries `<src>.ts`, then `<src>/index.ts` for directory-style imports.
/// Walk up from `from_dir` looking for a `shims/<package>.ts` file.
fn find_npm_shim(package_name: &str, from_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut dir = from_dir.to_path_buf();
    loop {
        let shims = dir.join("shims");
        // Direct match: shims/package-name.ts
        let shim = shims.join(format!("{}.ts", package_name));
        if shim.exists() { return Some(shim); }
        // Scoped packages: @scope/name → shims/@scope/name.ts
        // Also try shims/scope__name.ts as alternative flat layout
        if package_name.starts_with('@') {
            let flat = package_name.trim_start_matches('@').replace('/', "__");
            let flat_shim = shims.join(format!("{}.ts", flat));
            if flat_shim.exists() { return Some(flat_shim); }
        }
        // Directory-style: shims/package-name/index.ts
        let index_shim = shims.join(package_name).join("index.ts");
        if index_shim.exists() { return Some(index_shim); }
        if !dir.pop() { break; }
    }
    None
}

/// Read a simple string field from a package.json without a JSON parser dependency.
fn read_package_json_field(pkg_dir: &std::path::Path, field: &str) -> Option<String> {
    let content = std::fs::read_to_string(pkg_dir.join("package.json")).ok()?;
    let needle = format!("\"{}\"", field);
    let pos = content.find(&needle)?;
    let after = &content[pos + needle.len()..];
    let colon = after.find(':')? + 1;
    let after_colon = after[colon..].trim_start();
    if after_colon.starts_with('"') {
        let inner = &after_colon[1..];
        let end = inner.find('"')?;
        Some(inner[..end].to_string())
    } else {
        None
    }
}

/// Parse the "exports" field from package.json to find the best JS entry point.
/// Handles: string, { ".": string }, { ".": { "import": ..., "default": ..., "require": ... } }
fn read_package_exports_entry(pkg_dir: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(pkg_dir.join("package.json")).ok()?;
    // Find the "exports" key
    let exports_key = "\"exports\"";
    let pos = content.find(exports_key)?;
    let after_key = &content[pos + exports_key.len()..];
    let colon_pos = after_key.find(':')?;
    let after_colon = after_key[colon_pos + 1..].trim_start();

    if after_colon.starts_with('"') {
        // "exports": "./index.js"
        let inner = &after_colon[1..];
        let end = inner.find('"')?;
        return Some(inner[..end].to_string());
    }

    if after_colon.starts_with('{') {
        // Object form — look for "." entry
        let obj_content = after_colon;
        // Find the "." key
        let dot_key = "\".\"";
        if let Some(dot_pos) = obj_content.find(dot_key) {
            let after_dot = &obj_content[dot_pos + dot_key.len()..];
            let colon2 = after_dot.find(':')?;
            let after_colon2 = after_dot[colon2 + 1..].trim_start();
            if after_colon2.starts_with('"') {
                // { ".": "./index.js" }
                let inner = &after_colon2[1..];
                let end = inner.find('"')?;
                return Some(inner[..end].to_string());
            }
            if after_colon2.starts_with('{') {
                // { ".": { "import": ..., "default": ..., "require": ... } }
                // Prefer: import > default > require
                for condition in &["\"import\"", "\"default\"", "\"require\""] {
                    if let Some(cond_pos) = after_colon2.find(condition) {
                        let after_cond = &after_colon2[cond_pos + condition.len()..];
                        if let Some(cpos) = after_cond.find(':') {
                            let val = after_cond[cpos + 1..].trim_start();
                            if val.starts_with('"') {
                                let inner = &val[1..];
                                if let Some(end) = inner.find('"') {
                                    return Some(inner[..end].to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

/// Resolve a TypeScript source file within a package directory.
/// Tries multiple strategies: src/index.ts, "source" field, main-based heuristic,
/// and finally JS fallback via "module", "exports", "main" fields and index files.
fn resolve_ts_in_pkg(pkg_dir: &std::path::Path, sub_path: &str) -> Option<std::path::PathBuf> {
    if sub_path.is_empty() {
        // No sub-path: look for package root TypeScript source
        // 1. src/index.ts (common for monorepo packages)
        let src_idx = pkg_dir.join("src").join("index.ts");
        if src_idx.exists() { return Some(src_idx); }
        // 2. "source" field in package.json
        if let Some(src_field) = read_package_json_field(pkg_dir, "source") {
            let src_path = pkg_dir.join(&src_field);
            if src_path.exists() { return Some(src_path); }
        }
        // 3. "main" field: if it's dist/foo.js, look for src/foo.ts
        if let Some(main) = read_package_json_field(pkg_dir, "main") {
            if main.contains("dist/") || main.contains("/dist/") {
                let ts_main = main
                    .replace("dist/", "src/")
                    .replace(".js", ".ts")
                    .replace(".cjs", ".ts")
                    .replace(".mjs", ".ts");
                let ts_path = pkg_dir.join(&ts_main);
                if ts_path.exists() { return Some(ts_path); }
            }
        }
        // 4. index.ts at root
        let root_idx = pkg_dir.join("index.ts");
        if root_idx.exists() { return Some(root_idx); }

        // JS fallbacks: 5. "module" field (ESM build)
        if let Some(module_field) = read_package_json_field(pkg_dir, "module") {
            let p = pkg_dir.join(&module_field);
            if p.exists() { return Some(p); }
        }
        // 6. "exports" field
        if let Some(entry) = read_package_exports_entry(pkg_dir) {
            let p = pkg_dir.join(&entry);
            if p.exists() { return Some(p); }
        }
        // 7. "main" field (may be .js, .mjs, .cjs)
        if let Some(main) = read_package_json_field(pkg_dir, "main") {
            let p = pkg_dir.join(&main);
            if p.exists() { return Some(p); }
        }
        // 8. index.mjs / index.js / index.cjs
        for idx in &["index.mjs", "index.js", "index.cjs"] {
            let p = pkg_dir.join(idx);
            if p.exists() { return Some(p); }
        }
        None
    } else {
        // Has sub-path: try to find the TypeScript file for that sub-path
        // e.g., "dist/foo/bar" → "src/foo/bar.ts"; "lib/foo" → "src/foo.ts"
        let base = sub_path
            .trim_end_matches(".js")
            .trim_end_matches(".cjs")
            .trim_end_matches(".mjs");
        // Strip known compiled-output prefixes to get the logical name
        let logical = base
            .trim_start_matches("dist/")
            .trim_start_matches("lib/")
            .trim_start_matches("esm/");
        // Try TypeScript candidates in priority order: src/<logical>.ts, <base>.ts, <logical>.ts
        for prefix in &["src", ""] {
            for name in &[logical, base] {
                let path = if prefix.is_empty() {
                    pkg_dir.join(format!("{}.ts", name))
                } else {
                    pkg_dir.join(prefix).join(format!("{}.ts", name))
                };
                if path.exists() { return Some(path); }
                let idx = if prefix.is_empty() {
                    pkg_dir.join(name).join("index.ts")
                } else {
                    pkg_dir.join(prefix).join(name).join("index.ts")
                };
                if idx.exists() { return Some(idx); }
            }
        }
        // JS fallback: try the sub-path as-is
        let sub_path_direct = pkg_dir.join(sub_path);
        if sub_path_direct.exists() { return Some(sub_path_direct); }
        // Try with .js extension
        let sub_js = pkg_dir.join(format!("{}.js", base));
        if sub_js.exists() { return Some(sub_js); }
        let sub_mjs = pkg_dir.join(format!("{}.mjs", base));
        if sub_mjs.exists() { return Some(sub_mjs); }
        let sub_cjs = pkg_dir.join(format!("{}.cjs", base));
        if sub_cjs.exists() { return Some(sub_cjs); }
        None
    }
}

/// Resolve a bare npm package specifier via node_modules lookup.
/// Walks up the directory tree from `from_dir` to find the first `node_modules/` that contains
/// the package. Prefers TypeScript source over compiled JS.
fn resolve_npm_package(spec: &str, from_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    // Use absolute path for the walk so relative `from_dir` values (like ".") resolve correctly.
    let from_dir = if from_dir.is_relative() {
        std::env::current_dir().ok()?.join(from_dir)
    } else {
        from_dir.to_path_buf()
    };
    // Split spec into package name and optional sub-path.
    // e.g., "@vendure/core"         → pkg="@vendure/core", sub=""
    //        "typeorm/entity"        → pkg="typeorm", sub="entity"
    //        "@nestjs/common/utils"  → pkg="@nestjs/common", sub="utils"
    let (pkg_name, sub_path) = if spec.starts_with('@') {
        // Scoped package: first two segments are the name
        let parts: Vec<&str> = spec.splitn(3, '/').collect();
        if parts.len() >= 2 {
            let name = format!("{}/{}", parts[0], parts[1]);
            let sub = if parts.len() == 3 { parts[2] } else { "" };
            (name, sub.to_string())
        } else {
            (spec.to_string(), String::new())
        }
    } else {
        // Unscoped package: first segment is the name
        if let Some(slash) = spec.find('/') {
            (spec[..slash].to_string(), spec[slash+1..].to_string())
        } else {
            (spec.to_string(), String::new())
        }
    };

    // Walk up directory tree looking for node_modules/<pkg_name>
    let mut dir = from_dir.to_path_buf();
    loop {
        let pkg_dir = dir.join("node_modules").join(&pkg_name);
        if pkg_dir.exists() {
            // Resolve symlinks to get the real package directory
            let real_pkg = std::fs::canonicalize(&pkg_dir).unwrap_or(pkg_dir);
            if let Some(path) = resolve_ts_in_pkg(&real_pkg, &sub_path) {
                tracing::debug!("npm resolved: {} → {}", spec, path.display());
                return Some(path);
            } else {
                tracing::warn!("npm package '{}' found but no source entry found", spec);
                return None;
            }
        }
        if !dir.pop() { break; }
    }
    tracing::warn!("npm package '{}' not found in any node_modules", spec);
    None
}

fn resolve_local_import(src: &str, base_dir: &std::path::Path) -> Option<std::path::PathBuf> {
    // Bare specifiers (npm package names): look for shims, then node_modules.
    if !src.starts_with("./") && !src.starts_with("../") {
        if src.starts_with("node:") {
            // Strip the "node:" prefix and look for a shim file
            let bare = &src["node:".len()..];
            if let Some(shim) = find_npm_shim(bare, base_dir) {
                return Some(shim);
            }
            return None;
        }
        // 1. Shim files take priority (allows overriding any package)
        if let Some(shim) = find_npm_shim(src, base_dir) {
            return Some(shim);
        }
        // 2. node_modules lookup (prefer TypeScript source)
        return resolve_npm_package(src, base_dir);
    }
    let joined = base_dir.join(src);
    // Normalize away `.` components in the path.
    let mut p = std::path::PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => { p.pop(); }
            c => p.push(c),
        }
    }
    if p.extension().is_none() {
        // TypeScript source takes priority.
        let ts = p.with_extension("ts");
        if ts.exists() { return Some(ts); }
        let idx = p.join("index.ts");
        if idx.exists() { return Some(idx); }
        // Fall back to JavaScript (CJS packages use .js / /index.js).
        let js = p.with_extension("js");
        if js.exists() { return Some(js); }
        let idx_js = p.join("index.js");
        if idx_js.exists() { return Some(idx_js); }
        let mjs = p.with_extension("mjs");
        if mjs.exists() { return Some(mjs); }
        let cjs = p.with_extension("cjs");
        if cjs.exists() { return Some(cjs); }
        // Nothing found — return the .ts path to produce a clear "not found" warning.
        return Some(ts);
    }
    if p.extension().map_or(false, |e| e != "ts") {
        // First try appending .ts (handles multi-part names like "bind.decorator" → "bind.decorator.ts").
        let mut appended = p.clone().into_os_string();
        appended.push(".ts");
        let appended_path = std::path::PathBuf::from(appended);
        if appended_path.exists() { return Some(appended_path); }
        // Then try replacing the last extension ("bind.decorator" → "bind.ts").
        let ts = p.with_extension("ts");
        if ts.exists() { return Some(ts); }
        // Also try directory index.
        let idx = p.join("index.ts");
        if idx.exists() { return Some(idx); }
        // Fall back to .js variants.
        let js = p.with_extension("js");
        if js.exists() { return Some(js); }
        let idx_js = p.join("index.js");
        if idx_js.exists() { return Some(idx_js); }
    }
    Some(p)
}

/// Collect require() specifier strings from a CJS program.
fn collect_require_specs(program: &oxc_ast::ast::Program<'_>) -> Vec<String> {
    use oxc_ast::ast::{Statement, Expression};
    let mut specs = Vec::new();
    for stmt in &program.body {
        collect_require_specs_stmt(stmt, &mut specs);
    }
    specs
}

fn collect_require_specs_stmt(stmt: &oxc_ast::ast::Statement<'_>, specs: &mut Vec<String>) {
    use oxc_ast::ast::{Statement, Expression};
    match stmt {
        Statement::VariableDeclaration(vd) => {
            for decl in &vd.declarations {
                if let Some(init) = &decl.init {
                    collect_require_specs_expr(init, specs);
                }
            }
        }
        Statement::ExpressionStatement(es) => {
            collect_require_specs_expr(&es.expression, specs);
        }
        Statement::BlockStatement(block) => {
            for s in &block.body {
                collect_require_specs_stmt(s, specs);
            }
        }
        Statement::IfStatement(if_stmt) => {
            collect_require_specs_stmt(&if_stmt.consequent, specs);
            if let Some(alt) = &if_stmt.alternate {
                collect_require_specs_stmt(alt, specs);
            }
        }
        Statement::ReturnStatement(ret) => {
            if let Some(arg) = &ret.argument {
                collect_require_specs_expr(arg, specs);
            }
        }
        _ => {}
    }
}

fn collect_require_specs_expr(expr: &oxc_ast::ast::Expression<'_>, specs: &mut Vec<String>) {
    use oxc_ast::ast::{Expression, StringLiteral};
    match expr {
        Expression::CallExpression(call) => {
            if let Expression::Identifier(id) = &call.callee {
                if id.name == "require" {
                    if let Some(arg) = call.arguments.first() {
                        if let Some(Expression::StringLiteral(s)) = arg.as_expression() {
                            specs.push(s.value.to_string());
                        }
                    }
                }
            }
            // Recurse into arguments
            for arg in &call.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_require_specs_expr(e, specs);
                }
            }
        }
        Expression::AssignmentExpression(assign) => {
            collect_require_specs_expr(&assign.right, specs);
        }
        _ => {}
    }
}

struct CjsExportNames {
    functions: Vec<String>,
    values: Vec<String>,
}

/// Scan a CJS program for its exports (exports.foo = ..., module.exports.foo = ..., module.exports = {...}).
fn scan_cjs_exports(program: &oxc_ast::ast::Program<'_>) -> CjsExportNames {
    use oxc_ast::ast::{Statement, Expression, AssignmentTarget, SimpleAssignmentTarget};
    let mut functions = Vec::new();
    let mut values = Vec::new();

    for stmt in &program.body {
        let expr = match stmt {
            Statement::ExpressionStatement(es) => &es.expression,
            _ => continue,
        };

        match expr {
            Expression::AssignmentExpression(assign) => {
                match &assign.left {
                    AssignmentTarget::StaticMemberExpression(member) => {
                        // exports.foo = ... or module.exports.foo = ...
                        let is_exports_direct = if let Expression::Identifier(id) = &member.object {
                            id.name == "exports"
                        } else { false };

                        let is_module_exports = if let Expression::StaticMemberExpression(inner) = &member.object {
                            if let Expression::Identifier(id) = &inner.object {
                                id.name == "module" && inner.property.name == "exports"
                            } else { false }
                        } else { false };

                        if is_exports_direct || is_module_exports {
                            let export_name = member.property.name.to_string();
                            let rhs = Lowerer::strip_ts_casts(&assign.right);
                            match rhs {
                                Expression::FunctionExpression(_) |
                                Expression::ArrowFunctionExpression(_) => {
                                    functions.push(export_name);
                                }
                                _ => {
                                    values.push(export_name);
                                }
                            }
                        }

                        // module.exports = { foo: ..., bar: ... }
                        let is_module_exports_root = if let Expression::Identifier(id) = &member.object {
                            id.name == "module" && member.property.name == "exports"
                        } else { false };

                        if is_module_exports_root {
                            let rhs = Lowerer::strip_ts_casts(&assign.right);
                            if let Expression::ObjectExpression(obj) = rhs {
                                for prop in &obj.properties {
                                    use oxc_ast::ast::ObjectPropertyKind;
                                    if let ObjectPropertyKind::ObjectProperty(op) = prop {
                                        use oxc_ast::ast::PropertyKey;
                                        let key = match &op.key {
                                            PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
                                            PropertyKey::StringLiteral(s) => Some(s.value.to_string()),
                                            _ => None,
                                        };
                                        if let Some(name) = key {
                                            let v = Lowerer::strip_ts_casts(&op.value);
                                            match v {
                                                Expression::FunctionExpression(_) |
                                                Expression::ArrowFunctionExpression(_) => {
                                                    functions.push(name);
                                                }
                                                _ => {
                                                    values.push(name);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    CjsExportNames { functions, values }
}

/// Lower a CJS module into the MLIR module.
/// Handles:
/// - Top-level function declarations → lower as MLIR functions
/// - exports.foo = function(...) { body } → emits @foo as a top-level MLIR function
/// - exports.foo = identifier (referring to a fn decl) → register identifier as the MLIR name
/// - exports.foo = expr → emits in a module init function via ts_set_module_global
/// - module.exports = { foo: fn, bar: val } → same treatment
/// - Other statements (vars, non-export code) → lowered as module-level init code
fn lower_cjs_module<'c, 'm>(
    lowerer: &mut Lowerer<'c, 'm>,
    program: &oxc_ast::ast::Program<'_>,
    module_name: &str,
) -> Result<()> {
    use oxc_ast::ast::{Statement, Expression, AssignmentTarget, Declaration};

    // Collect function signatures first (hoisting pass).
    // For CJS modules we call a restricted version that only processes FunctionDeclaration nodes
    // (not VariableDeclaration with const-function-expressions). This avoids polluting the global
    // `self.funcs` namespace with module-internal helpers (e.g. `const parse = (query) => {...}`
    // in pg-protocol/serializer.js) that would shadow functions of the same name from other modules.
    lowerer.collect_cjs_function_signatures(program);
    lowerer.collect_class_definitions(program);

    // Temporarily add all module-level const/let/var binding names to module_global_names so that
    // class methods and closures defined within this module can access them via scope injection.
    // These names are removed at the end of this function to avoid polluting other modules.
    let mut temp_module_globals: Vec<String> = Vec::new();
    for stmt in &program.body {
        if let Statement::VariableDeclaration(vd) = stmt {
            for decl in &vd.declarations {
                if let BindingPattern::BindingIdentifier(id) = &decl.id {
                    let name = id.name.to_string();
                    if !lowerer.module_global_names.contains(&name) {
                        lowerer.module_global_names.insert(name.clone());
                        temp_module_globals.push(name);
                    }
                }
            }
        }
    }

    // Step 1: Lower all top-level function declarations (they may be referenced by exports.foo = fnName)
    for stmt in &program.body {
        match stmt {
            Statement::FunctionDeclaration(func) => {
                if !func.declare {
                    lowerer.lower_function_declaration(func)?;
                }
            }
            _ => {}
        }
    }

    // Step 1b: Lower class declarations (CJS modules often export classes like `module.exports = Client`)
    // Must come after functions but before export processing so emitted_functions contains class names.
    for stmt in &program.body {
        if let Statement::ClassDeclaration(class) = stmt {
            if !class.declare {
                lowerer.lower_class_declaration(class)?;
            }
        }
    }

    // Build a set of all top-level function declaration names for identifier-export detection
    let top_level_fns: std::collections::HashSet<String> = program.body.iter().filter_map(|stmt| {
        if let Statement::FunctionDeclaration(func) = stmt {
            func.id.as_ref().map(|id| id.name.to_string())
        } else {
            None
        }
    }).collect();

    // Helper closure: register an export binding given the export name and RHS expression
    // Returns true if the export was handled as a function (false means treat as value)
    let register_fn_export = |lowerer: &mut Lowerer<'c, 'm>,
                               export_name: &str,
                               rhs: &Expression<'_>,
                               top_fns: &std::collections::HashSet<String>| -> Result<bool> {
        let inner = Lowerer::strip_ts_casts(rhs);
        match inner {
            Expression::FunctionExpression(func_expr) => {
                let fn_name = if lowerer.emitted_functions.contains(export_name) {
                    format!("__shim_{}_{}", module_name, export_name)
                } else {
                    export_name.to_string()
                };
                lowerer.lower_arrow_or_func_expr_as(func_expr.as_ref(), &fn_name)?;
                lowerer.emitted_functions.insert(fn_name.clone());
                lowerer.module_exports
                    .entry(module_name.to_string())
                    .or_insert_with(HashMap::new)
                    .insert(export_name.to_string(), fn_name);
                Ok(true)
            }
            Expression::ArrowFunctionExpression(arrow) => {
                let fn_name = if lowerer.emitted_functions.contains(export_name) {
                    format!("__shim_{}_{}", module_name, export_name)
                } else {
                    export_name.to_string()
                };
                let params: Vec<&oxc_ast::ast::FormalParameter<'_>> =
                    arrow.params.items.iter().collect();
                let rest_name = arrow.params.rest.as_ref()
                    .and_then(|r| if let BindingPattern::BindingIdentifier(id) = &r.rest.argument {
                        Some(id.name.to_string())
                    } else { None });
                lowerer.lower_named_function(
                    &fn_name, &params,
                    rest_name.as_deref(),
                    Some(&arrow.body),
                    Some(arrow.r#async),
                    arrow.expression,
                )?;
                lowerer.emitted_functions.insert(fn_name.clone());
                lowerer.module_exports
                    .entry(module_name.to_string())
                    .or_insert_with(HashMap::new)
                    .insert(export_name.to_string(), fn_name);
                Ok(true)
            }
            Expression::Identifier(id) => {
                // exports.foo = someFn — check if it references a top-level function
                let fn_id_name = id.name.to_string();
                if top_fns.contains(&fn_id_name) || lowerer.emitted_functions.contains(&fn_id_name) {
                    // The function is already emitted under fn_id_name.
                    // Register export_name as an alias pointing to fn_id_name.
                    if export_name != fn_id_name {
                        // Also register the function under the export name if not yet taken
                        if !lowerer.emitted_functions.contains(export_name) {
                            // Create a thin wrapper function that calls fn_id_name
                            let sig = lowerer.funcs.get(&fn_id_name).cloned();
                            if let Some(sig) = sig {
                                let n = sig.param_types.len();
                                let i64t = lowerer.i64_type();
                                let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
                                    (0..n).map(|_| (i64t, lowerer.loc)).collect();
                                let func_type = melior::ir::r#type::FunctionType::new(
                                    lowerer.ctx, &vec![i64t; n], &[i64t]
                                );
                                let region = Region::new();
                                let entry = region.append_block(Block::new(&param_specs));
                                let mut args: Vec<melior::ir::Value<'_, '_>> = (0..n)
                                    .map(|i| entry.argument(i).unwrap().into())
                                    .collect();
                                let result: melior::ir::Value<'_, '_> = entry.append_operation(
                                    melior::dialect::func::call(
                                        lowerer.ctx,
                                        melior::ir::attribute::FlatSymbolRefAttribute::new(lowerer.ctx, &fn_id_name),
                                        &args,
                                        &[i64t],
                                        lowerer.loc,
                                    )
                                ).result(0)?.into();
                                entry.append_operation(melior::dialect::func::r#return(&[result], lowerer.loc));
                                let fn_op = melior::dialect::func::func(
                                    lowerer.ctx,
                                    melior::ir::attribute::StringAttribute::new(lowerer.ctx, export_name),
                                    melior::ir::attribute::TypeAttribute::new(func_type.into()),
                                    region, &[(
                                        melior::ir::Identifier::new(lowerer.ctx, "sym_visibility"),
                                        melior::ir::attribute::StringAttribute::new(lowerer.ctx, "private").into(),
                                    )], lowerer.loc,
                                );
                                lowerer.module.body().append_operation(fn_op);
                                lowerer.emitted_functions.insert(export_name.to_string());
                                lowerer.funcs.insert(export_name.to_string(), sig);
                            }
                        }
                        lowerer.module_global_aliases.insert(export_name.to_string(), fn_id_name.clone());
                    }
                    lowerer.module_exports
                        .entry(module_name.to_string())
                        .or_insert_with(HashMap::new)
                        .insert(export_name.to_string(), export_name.to_string());
                    return Ok(true);
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    };

    // Step 2: Lower exported functions as top-level MLIR functions
    for stmt in &program.body {
        let Statement::ExpressionStatement(es) = stmt else { continue };
        let Expression::AssignmentExpression(assign) = &es.expression else { continue };

        match &assign.left {
            AssignmentTarget::StaticMemberExpression(member) => {
                let is_exports_direct = if let Expression::Identifier(id) = &member.object {
                    id.name == "exports"
                } else { false };
                let is_module_exports_prop = if let Expression::StaticMemberExpression(inner) = &member.object {
                    if let Expression::Identifier(id) = &inner.object {
                        id.name == "module" && inner.property.name == "exports"
                    } else { false }
                } else { false };

                if is_exports_direct || is_module_exports_prop {
                    let export_name = member.property.name.to_string();
                    let handled_as_fn = register_fn_export(lowerer, &export_name, &assign.right, &top_level_fns)?;
                    if !handled_as_fn {
                        // Non-function exports go to module globals
                        lowerer.module_global_names.insert(export_name);
                    }
                }

                // module.exports = { ... }
                let is_module_exports_root = if let Expression::Identifier(id) = &member.object {
                    id.name == "module" && member.property.name == "exports"
                } else { false };

                if is_module_exports_root {
                    let rhs = Lowerer::strip_ts_casts(&assign.right);
                    if let Expression::ObjectExpression(obj) = rhs {
                        for prop in &obj.properties {
                            use oxc_ast::ast::ObjectPropertyKind;
                            if let ObjectPropertyKind::ObjectProperty(op) = prop {
                                use oxc_ast::ast::PropertyKey;
                                let key = match &op.key {
                                    PropertyKey::StaticIdentifier(id) => Some(id.name.to_string()),
                                    PropertyKey::StringLiteral(s) => Some(s.value.to_string()),
                                    _ => None,
                                };
                                if let Some(export_name) = key {
                                    let handled = register_fn_export(lowerer, &export_name, &op.value, &top_level_fns)?;
                                    if !handled {
                                        lowerer.module_global_names.insert(export_name);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Emit module init for non-function exports and other module-level code
    lowerer.lower_cjs_module_init(program)?;

    // Remove the temp module globals we added so they don't bleed into other modules.
    for name in &temp_module_globals {
        lowerer.module_global_names.remove(name);
    }

    Ok(())
}

/// Collect local import paths declared in a parsed program, relative to `base_dir`.
/// Also collects dynamic `import('specifier')` calls so they are compiled ahead-of-time.
fn collect_local_imports(
    program: &oxc_ast::ast::Program<'_>,
    base_dir: &std::path::Path,
) -> Vec<std::path::PathBuf> {
    use oxc_ast::ast::ImportOrExportKind;
    let mut paths = Vec::new();
    for stmt in &program.body {
        let src_opt: Option<&str> = match stmt {
            Statement::ImportDeclaration(import) => {
                if import.import_kind == ImportOrExportKind::Type { None }
                else { Some(import.source.value.as_str()) }
            }
            Statement::ExportNamedDeclaration(exp) => {
                if exp.export_kind == ImportOrExportKind::Type { None }
                else { exp.source.as_ref().map(|s| s.value.as_str()) }
            }
            Statement::ExportAllDeclaration(exp) => {
                if exp.export_kind == ImportOrExportKind::Type { None }
                else { Some(exp.source.value.as_str()) }
            }
            _ => None,
        };
        if let Some(src) = src_opt {
            if let Some(p) = resolve_local_import(src, base_dir) {
                paths.push(p);
            }
        }
    }
    // Also collect dynamic import() calls: import('specifier')
    collect_dynamic_imports_stmts(&program.body, base_dir, &mut paths);
    paths
}

fn collect_dynamic_imports_stmts(
    stmts: &[oxc_ast::ast::Statement<'_>],
    base_dir: &std::path::Path,
    paths: &mut Vec<std::path::PathBuf>,
) {
    use oxc_ast::ast::{Statement, Expression};
    for stmt in stmts {
        match stmt {
            Statement::ExpressionStatement(es) => collect_dynamic_imports_expr(&es.expression, base_dir, paths),
            Statement::VariableDeclaration(vd) => {
                for decl in &vd.declarations {
                    if let Some(init) = &decl.init {
                        collect_dynamic_imports_expr(init, base_dir, paths);
                    }
                }
            }
            Statement::ReturnStatement(r) => {
                if let Some(arg) = &r.argument {
                    collect_dynamic_imports_expr(arg, base_dir, paths);
                }
            }
            Statement::BlockStatement(b) => collect_dynamic_imports_stmts(&b.body, base_dir, paths),
            Statement::IfStatement(i) => {
                collect_dynamic_imports_stmts(std::slice::from_ref(&i.consequent), base_dir, paths);
                if let Some(alt) = &i.alternate {
                    collect_dynamic_imports_stmts(std::slice::from_ref(alt), base_dir, paths);
                }
            }
            _ => {}
        }
    }
}

fn collect_dynamic_imports_expr(
    expr: &oxc_ast::ast::Expression<'_>,
    base_dir: &std::path::Path,
    paths: &mut Vec<std::path::PathBuf>,
) {
    use oxc_ast::ast::Expression;
    match expr {
        Expression::ImportExpression(import_expr) => {
            if let Expression::StringLiteral(s) = &import_expr.source {
                if let Some(p) = resolve_local_import(s.value.as_str(), base_dir) {
                    paths.push(p);
                }
            }
        }
        Expression::AwaitExpression(a) => collect_dynamic_imports_expr(&a.argument, base_dir, paths),
        Expression::CallExpression(c) => {
            collect_dynamic_imports_expr(&c.callee, base_dir, paths);
            for arg in &c.arguments {
                if let Some(e) = arg.as_expression() {
                    collect_dynamic_imports_expr(e, base_dir, paths);
                }
            }
        }
        Expression::AssignmentExpression(a) => collect_dynamic_imports_expr(&a.right, base_dir, paths),
        _ => {}
    }
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

    let Some((imported, is_cjs)) = load_import_static(path) else {
        tracing::warn!("failed to resolve import: {}", path.display());
        return Ok(());
    };

    // For CJS modules, detect require() specifiers and process them recursively.
    let base_dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
    if is_cjs {
        let require_specs = collect_require_specs(&imported);
        for spec in &require_specs {
            if let Some(dep_path) = resolve_local_import(spec, base_dir) {
                process_import_recursive(lowerer, &dep_path, visited)?;
            }
        }
        // Scan what this CJS module exports and lower it
        let cjs_exports = scan_cjs_exports(&imported);
        let module_name: String = path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        // Register non-function value exports as module globals (e.g. exports.VERSION = '1.0.0').
        // Function exports are emitted as MLIR functions by lower_cjs_module and registered in
        // lowerer.funcs — they must NOT go into module_global_names, because that would cause
        // ts_get_module_global lookups (returning UNDEFINED) instead of direct MLIR calls.
        for name in &cjs_exports.values {
            lowerer.module_global_names.insert(name.clone());
        }
        // Lower the CJS module code
        lower_cjs_module(lowerer, &imported, &module_name)?;
        return Ok(());
    }

    // Process transitive imports depth-first (ESM path).
    for sub_path in collect_local_imports(&imported, base_dir) {
        process_import_recursive(lowerer, &sub_path, visited)?;
    }

    // Register import aliases for this file: `import { X as Y }` adds "Y" → "X".
    for stmt in &imported.body {
        if let Statement::ImportDeclaration(import) = stmt {
            if let Some(specs) = &import.specifiers {
                for spec in specs {
                    if let ImportDeclarationSpecifier::ImportSpecifier(s) = spec {
                        let alias = s.local.name.to_string();
                        let original = s.imported.name().to_string();
                        if alias != original {
                            lowerer.module_global_aliases.insert(alias, original);
                        }
                    }
                }
            }
        }
    }

    // Register signatures and lower declarations for this file.
    lowerer.collect_function_signatures(&imported);
    lowerer.collect_class_definitions(&imported);
    lowerer.collect_enum_definitions(&imported);

    // Build a map: local_class_name → exported_alias_name from `export { Foo as Bar }` patterns.
    // This is needed so that when a class is re-exported under a different name, the MLIR functions
    // are generated under the alias name (e.g., `class Hono` exported as `HonoBase` → `__class_HonoBase_*`).
    let mut class_export_aliases: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for stmt in &imported.body {
        if let Statement::ExportNamedDeclaration(exp) = stmt {
            if exp.declaration.is_none() {
                for spec in &exp.specifiers {
                    use oxc_ast::ast::ExportSpecifier;
                    let local_name = spec.local.name().to_string();
                    let exported_name = spec.exported.name().to_string();
                    if local_name != exported_name {
                        class_export_aliases.insert(local_name, exported_name);
                    }
                }
            }
        }
    }

    for stmt in &imported.body {
        match stmt {
            Statement::ClassDeclaration(class) => {
                if let Some(id) = &class.id {
                    let original_name = id.name.to_string();
                    if let Some(alias) = class_export_aliases.get(&original_name) {
                        // Lower under the exported alias name (e.g., Hono → HonoBase)
                        lowerer.lower_class_declaration_with_name(alias, class)?;
                    } else {
                        lowerer.lower_class_declaration(class)?;
                    }
                }
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(Declaration::ClassDeclaration(class)) = &exp.declaration {
                    lowerer.lower_class_declaration(class)?;
                }
            }
            _ => {}
        }
    }
    // Derive a module name from the file path (e.g. "shims/path.ts" → "path") for
    // module-scoped naming when exported function names collide across shim modules.
    let module_name: String = path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    // Helper: lower a function from this module, using a prefixed name if the bare name
    // is already claimed by a previously loaded module. Record the mapping in module_exports.
    macro_rules! lower_with_module_tracking {
        ($func:expr) => {{
            let func = $func;
            if let Some(id) = &func.id {
                let orig_name = id.name.to_string();
                if !func.declare && func.body.is_some() {
                    let actual_name = if lowerer.emitted_functions.contains(&orig_name) {
                        // Name already claimed — use a module-prefixed variant.
                        format!("__shim_{}_{}", module_name, orig_name)
                    } else {
                        orig_name.clone()
                    };
                    if actual_name != orig_name {
                        lowerer.lower_function_declaration_as(func, &actual_name)?;
                    } else {
                        lowerer.lower_function_declaration(func)?;
                    }
                    lowerer.module_exports
                        .entry(module_name.clone())
                        .or_insert_with(HashMap::new)
                        .insert(orig_name, actual_name);
                } else {
                    lowerer.lower_function_declaration(func)?;
                }
            }
        }};
    }

    for stmt in &imported.body {
        match stmt {
            Statement::FunctionDeclaration(func) => {
                lower_with_module_tracking!(func);
            }
            Statement::ExportNamedDeclaration(exp) => {
                if let Some(Declaration::FunctionDeclaration(func)) = &exp.declaration {
                    lower_with_module_tracking!(func);
                }
            }
            Statement::ExportDefaultDeclaration(exp) => {
                use oxc_ast::ast::ExportDefaultDeclarationKind;
                if let ExportDefaultDeclarationKind::FunctionDeclaration(func) = &exp.declaration {
                    lower_with_module_tracking!(func);
                }
            }
            _ => {}
        }
    }
    lowerer.lower_module_const_functions(&imported)?;

    // Emit an init function for non-function module-level const declarations
    // (e.g. `export const METHODS = ['get', 'post', ...]`).  These cannot be
    // lowered as hoisted functions; instead we generate a `__init_module_N()`
    // function and call it at the start of `main`.
    lowerer.lower_imported_module_init(&imported)?;

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
        is_generator: false,
        enums: HashMap::new(),
        current_class: None,
        super_ctor: None,
        builtin_aliases: HashMap::new(),
        module_global_names: std::collections::HashSet::new(),
        module_global_aliases: HashMap::new(),
        module_exports: HashMap::new(),
        builtin_wrappers_emitted: std::collections::HashSet::new(),
        lowered_classes: std::collections::HashSet::new(),
        emitted_functions: std::collections::HashSet::new(),
        closure_env_indices: HashMap::new(),
        current_fn_params: std::collections::HashSet::new(),
        module_init_fns: Vec::new(),
        module_init_fn_count: 0,
        class_name_aliases: HashMap::new(),
        cell_vars: std::collections::HashSet::new(),
        cell_captures: std::collections::HashSet::new(),
        scalar_vars: std::collections::HashSet::new(),
        non_escaping_allocs: std::collections::HashSet::new(),
        arena_alloc_next: false,
        pending_label: None,
        addon_mode: cg.addon_mode,
        napi_exports: Vec::new(),
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
            let resolved_path = if src.starts_with("./") || src.starts_with("../") {
                // Resolve .ts extension
                let mut path = base_dir.join(src);
                if path.extension().is_none() {
                    path.set_extension("ts");
                } else if path.extension().map_or(false, |e| e != "ts") {
                    // Non-ts import (e.g., .js), try adding .ts
                    let ts_path = path.with_extension("ts");
                    if ts_path.exists() { path = ts_path; }
                }
                Some(path)
            } else {
                // Bare specifier or node: prefix — try shim resolution
                resolve_local_import(src, base_dir)
            };
            if let Some(path) = resolved_path {
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
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                Some(s.local.name.to_string())
                            }
                        }
                    }).collect()
                } else {
                    Vec::new()
                };
                local_imports.push((path, names));
            }
            // Unresolved external imports (npm packages without shims) are silently skipped.
        }
    }

    // Process all local imports recursively (handles transitive dependencies).
    let mut visited: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    for (import_path, _names) in &local_imports {
        process_import_recursive(&mut lowerer, import_path, &mut visited)?;
    }

    // Register import aliases for the main program.
    for stmt in &program.body {
        if let Statement::ImportDeclaration(import) = stmt {
            if let Some(specs) = &import.specifiers {
                let src = import.source.value.as_str();
                // Derive module name from source (e.g. "node:path" → "path", "./foo/bar" → "bar").
                let src_module_name: String = if src.starts_with("node:") {
                    src["node:".len()..].to_string()
                } else {
                    src.rsplit('/').next().unwrap_or(src)
                        .trim_end_matches(".ts").to_string()
                };
                for spec in specs {
                    match spec {
                        ImportDeclarationSpecifier::ImportSpecifier(s) => {
                            let alias = s.local.name.to_string();
                            let original = s.imported.name().to_string();
                            // Resolve to the actual MLIR function name via module_exports.
                            // This handles the case where two modules export functions with the
                            // same name (e.g. util::format and path::format).
                            let actual_name = lowerer.module_exports
                                .get(&src_module_name)
                                .and_then(|m| m.get(&original))
                                .cloned()
                                .unwrap_or_else(|| original.clone());
                            // Always register the alias (even if alias == original) when
                            // the actual MLIR name differs from the export name.
                            if alias != actual_name || original != actual_name {
                                lowerer.module_global_aliases.insert(alias, actual_name);
                            }
                        }
                        ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                            // `import * as X from 'mod'` — map the local name X to the
                            // module's default-export global (the bare module name).
                            let local = s.local.name.to_string();
                            let bare = if src.starts_with("node:") {
                                src["node:".len()..].to_string()
                            } else {
                                // For bare specifiers use the last path segment.
                                src.rsplit('/').next().unwrap_or(src).to_string()
                            };
                            if local != bare {
                                lowerer.module_global_aliases.insert(local, bare);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
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
                    // In addon mode, record every exported function for __napi_init.
                    if lowerer.addon_mode {
                        if let Some(id) = &func.id {
                            let fn_name = id.name.to_string();
                            let sig = lowerer.funcs.get(&fn_name).cloned();
                            let arity = sig.map_or(0, |s| s.param_types.len());
                            lowerer.napi_exports.push((fn_name, arity));
                        }
                    }
                    lowerer.lower_function_declaration(func)?;
                }
            }
            Statement::ExportDefaultDeclaration(export) => {
                use oxc_ast::ast::ExportDefaultDeclarationKind;
                if let ExportDefaultDeclarationKind::FunctionDeclaration(func) = &export.declaration {
                    if lowerer.addon_mode {
                        if let Some(id) = &func.id {
                            let fn_name = id.name.to_string();
                            let sig = lowerer.funcs.get(&fn_name).cloned();
                            let arity = sig.map_or(0, |s| s.param_types.len());
                            lowerer.napi_exports.push((fn_name, arity));
                        }
                    }
                    lowerer.lower_function_declaration(func)?;
                }
            }
            _ => {}
        }
    }

    // Pass 2c – lower module-level const arrow/function declarations as hoisted functions.
    lowerer.lower_module_const_functions(program)?;

    // Pass 3 – generate entry point.
    if lowerer.addon_mode {
        // Addon mode: generate __napi_init() instead of main().
        lowerer.lower_napi_init_function(program)?;
    } else {
        // Normal mode: generate the implicit main().
        lowerer.lower_main_function(program)?;
    }

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
    /// Whether the function currently being lowered is a generator (`function*`).
    /// When true, `yield` expressions push to a TsArray named `__generator_yields`.
    is_generator: bool,
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
    /// Maps import alias → original export name for cross-module aliased imports.
    /// E.g. `import { MODULE_METADATA as metadataConstants }` adds "metadataConstants" → "MODULE_METADATA".
    /// Used during Identifier resolution so aliased imports resolve to the correct module global.
    module_global_aliases: HashMap<String, String>,
    /// Per-module export tables: module_name → { exported_name → actual_mlir_name }.
    /// Tracks the actual MLIR function name used for each export (may differ from the export name
    /// when there's a naming collision between modules, e.g. util::format vs path::format).
    module_exports: HashMap<String, HashMap<String, String>>,
    /// Tracks which built-in wrapper MLIR functions have already been emitted.
    builtin_wrappers_emitted: std::collections::HashSet<String>,
    /// Tracks which classes have already been lowered (to prevent re-emission from duplicate imports).
    lowered_classes: std::collections::HashSet<String>,
    /// Tracks which user-defined functions have already been emitted (to prevent redefinition errors
    /// when multiple imported modules declare the same helper function, e.g. `isPromise`).
    emitted_functions: std::collections::HashSet<String>,
    /// When inside a closure body with captures, maps captured variable name → env array index.
    /// Used to write back mutations to captured variables into the env array.
    closure_env_indices: HashMap<String, usize>,
    /// Parameter names of the function currently being lowered.
    /// `lower_return_statement` skips these so it does not release borrowed refs —
    /// the call site's post-call ts_release_val is the one that balances the pre-call
    /// ts_retain_val done by lower_expression for each argument.
    current_fn_params: std::collections::HashSet<String>,
    /// Names of module-init MLIR functions generated for imported files.
    /// Called at the start of `main` to initialize imported module-level const values.
    module_init_fns: Vec<String>,
    /// Counter for generating unique module init function names.
    module_init_fn_count: usize,
    /// Maps original class name → lowered alias name when a class is exported under a different name.
    /// E.g. hono-base.ts: `class Hono` exported as `HonoBase` → maps "Hono" → "HonoBase".
    /// Used so that `new Hono(...)` inside hono-base.ts resolves to `__class_HonoBase_constructor`.
    class_name_aliases: HashMap<String, String>,
    /// Local variables in the current function body that are "cell-ified":
    /// they are stored as single-element TsArrays so closures can mutate them
    /// and the outer scope sees the updated value.
    cell_vars: std::collections::HashSet<String>,
    /// Variables captured from an outer scope that are cells (set when entering a closure body).
    cell_captures: std::collections::HashSet<String>,
    /// Local `const` variables in the current function that are provably scalar (TAG_INT, TAG_BOOL,
    /// TAG_NULL, TAG_UNDEFINED).  `ts_retain_val`/`ts_release_val` calls on these are elided since
    /// both are no-ops for non-pointer values — we just skip the call overhead (~3–5 ns each).
    /// Saved/restored around each nested function body (same pattern as `cell_vars`).
    scalar_vars: std::collections::HashSet<String>,
    /// Local `const x = {}` / `const x = []` declarations in the current function body that
    /// are provably non-escaping (never returned, never passed as an argument to unknown functions,
    /// never stored into heap objects, never captured by closures).  These can be allocated on
    /// the fiber bump arena rather than the heap, eliminating ARC overhead entirely.
    /// Saved/restored around each nested function body (same pattern as `scalar_vars`).
    non_escaping_allocs: std::collections::HashSet<String>,
    /// Set to `true` immediately before lowering the initializer of a non-escaping variable
    /// declaration, so `lower_object_expression`/`lower_array_expression` know to emit
    /// `ts_obj_new_arena`/`ts_arr_new_arena` instead of `ts_obj_new`/`ts_arr_new`.
    /// Reset to `false` after the initializer is lowered.
    arena_alloc_next: bool,
    /// When a `LabeledStatement` wraps a loop/switch, this holds the label name so the
    /// loop-lowering code can attach it to the `inner_loops` entry it creates.
    pending_label: Option<String>,
    /// Whether we are compiling in Node.js addon mode.
    /// When true, `__napi_init()` is generated instead of `main()`.
    addon_mode: bool,
    /// Collected (fn_name, arity) pairs for each `export function` in the main program.
    /// Populated during Pass 2b; consumed by `lower_napi_init_function`.
    napi_exports: Vec<(String, usize)>,
}

impl<'c, 'm> Lowerer<'c, 'm> {
    /// Returns true if `name` is a cell variable in the current function context
    /// (either a locally-cellified var or a cell capture from outer scope).
    #[inline]
    pub(crate) fn is_cell_var(&self, name: &str) -> bool {
        self.cell_vars.contains(name) || self.cell_captures.contains(name)
    }

    /// Returns true if the expression's result is provably a scalar NaN-box value
    /// (TAG_INT, TAG_BOOL, TAG_NULL, TAG_UNDEFINED) — never a heap pointer.
    /// When true, `ts_retain_val`/`ts_release_val` are no-ops and their call overhead
    /// can be elided.  Also checks `scalar_vars` for known-scalar identifiers.
    pub(crate) fn expr_result_is_scalar(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Identifier(id) => {
                let n = id.name.as_str();
                n == "undefined" || n == "NaN" || n == "Infinity"
                    || self.scalar_vars.contains(n)
            }
            _ => expr_is_definitely_scalar(expr),
        }
    }

    /// Read the actual value from a cell pointer.
    /// `cell_ptr_val` is the scope value (a TsArray* wrapped in TsVal).
    /// Returns a retained reference to the inner value (ts_arr_get retains).
    pub(crate) fn cell_read<'b>(
        &mut self,
        cell_ptr_val: melior::ir::Value<'c, 'b>,
        block: melior::ir::BlockRef<'c, 'b>,
    ) -> anyhow::Result<melior::ir::Value<'c, 'b>> {
        let i64_type = self.i64_type();
        let i32_type = self.i32_type();
        let cell_i64 = self.ensure_i64(cell_ptr_val, block)?;
        let idx_zero: melior::ir::Value<'c, 'b> = block.append_operation(
            melior::dialect::arith::constant(
                self.ctx,
                melior::ir::attribute::IntegerAttribute::new(i32_type, 0).into(),
                self.loc,
            )
        ).result(0)?.into();
        let val: melior::ir::Value<'c, 'b> = block.append_operation(melior::dialect::func::call(
            self.ctx,
            melior::ir::attribute::FlatSymbolRefAttribute::new(self.ctx, "ts_arr_get"),
            &[cell_i64, idx_zero],
            &[i64_type],
            self.loc,
        )).result(0)?.into();
        Ok(val)
    }

    /// Write a value to a cell.
    /// Calls ts_arr_set(cell_ptr, 0, val). ts_arr_set retains val internally.
    pub(crate) fn cell_write<'b>(
        &mut self,
        cell_ptr_val: melior::ir::Value<'c, 'b>,
        new_val: melior::ir::Value<'c, 'b>,
        block: melior::ir::BlockRef<'c, 'b>,
    ) -> anyhow::Result<()> {
        let i32_type = self.i32_type();
        let cell_i64 = self.ensure_i64(cell_ptr_val, block)?;
        let new_i64 = self.ensure_i64(new_val, block)?;
        let idx_zero: melior::ir::Value<'c, 'b> = block.append_operation(
            melior::dialect::arith::constant(
                self.ctx,
                melior::ir::attribute::IntegerAttribute::new(i32_type, 0).into(),
                self.loc,
            )
        ).result(0)?.into();
        block.append_operation(melior::dialect::func::call(
            self.ctx,
            melior::ir::attribute::FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
            &[cell_i64, idx_zero, new_i64],
            &[],
            self.loc,
        ));
        Ok(())
    }

    /// Allocate a single-element TsArray cell holding `initial_val`.
    /// Returns the cell pointer (i64). ts_arr_set retains `initial_val` for the cell;
    /// the caller's copy of `initial_val` is released (ownership transferred to cell).
    pub(crate) fn alloc_cell<'b>(
        &mut self,
        initial_val: melior::ir::Value<'c, 'b>,
        block: melior::ir::BlockRef<'c, 'b>,
    ) -> anyhow::Result<melior::ir::Value<'c, 'b>> {
        let i64_type = self.i64_type();
        let i32_type = self.i32_type();
        let cap_one: melior::ir::Value<'c, 'b> = block.append_operation(
            melior::dialect::arith::constant(
                self.ctx,
                melior::ir::attribute::IntegerAttribute::new(i32_type, 1).into(),
                self.loc,
            )
        ).result(0)?.into();
        let cell: melior::ir::Value<'c, 'b> = block.append_operation(melior::dialect::func::call(
            self.ctx,
            melior::ir::attribute::FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
            &[cap_one],
            &[i64_type],
            self.loc,
        )).result(0)?.into();
        let init_i64 = self.ensure_i64(initial_val, block)?;
        let idx_zero: melior::ir::Value<'c, 'b> = block.append_operation(
            melior::dialect::arith::constant(
                self.ctx,
                melior::ir::attribute::IntegerAttribute::new(i32_type, 0).into(),
                self.loc,
            )
        ).result(0)?.into();
        block.append_operation(melior::dialect::func::call(
            self.ctx,
            melior::ir::attribute::FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
            &[cell, idx_zero, init_i64],
            &[],
            self.loc,
        ));
        // ts_arr_set retains init_val for the cell; release our owned copy.
        block.append_operation(melior::dialect::func::call(
            self.ctx,
            melior::ir::attribute::FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
            &[init_i64],
            &[],
            self.loc,
        ));
        Ok(cell)
    }

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
            &[(
                Identifier::new(self.ctx, "sym_visibility"),
                StringAttribute::new(self.ctx, "private").into(),
            )],
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

        let mut declared_names: Vec<String> = Vec::new();
        let mut add_func = |name: &str, params: &[Type<'c>], results: &[Type<'c>]| {
            let op = func::func(
                self.ctx,
                StringAttribute::new(self.ctx, name),
                TypeAttribute::new(FunctionType::new(self.ctx, params, results).into()),
                Region::new(),
                private,
                self.loc,
            );
            self.module.body().append_operation(op);
            declared_names.push(name.to_string());
        };

        add_func("__ts_console_log_i32", &[i32_type], &[]);
        add_func("__ts_console_log_val", &[i64_type], &[]);
        add_func("ts_retain_val", &[i64_type], &[]);
        add_func("ts_retain", &[ptr_type], &[]);
        add_func("ts_release", &[ptr_type, ptr_type], &[]);
        add_func("ts_release_val", &[i64_type], &[]);
        
        add_func("ts_obj_new", &[], &[i64_type]);
        add_func("ts_obj_new_arena", &[], &[i64_type]);
        add_func("ts_obj_get", &[i64_type, ptr_type], &[i64_type]);
        add_func("ts_obj_set", &[i64_type, ptr_type, i64_type], &[]);

        add_func("ts_arr_new", &[i32_type], &[i64_type]);
        add_func("ts_arr_new_arena", &[i32_type], &[i64_type]);
        add_func("ts_arr_get", &[i64_type, i32_type], &[i64_type]);
        add_func("ts_arr_set", &[i64_type, i32_type, i64_type], &[]);
        add_func("ts_arr_len", &[i64_type], &[i64_type]);
        add_func("ts_iterable_len", &[i64_type], &[i64_type]);
        add_func("ts_iterable_get", &[i64_type, i32_type], &[i64_type]);

        add_func("ts_string_new", &[ptr_type], &[i64_type]);
        add_func("ts_string_concat", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_add", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_sub", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_mul", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_div", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_mod",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_pow",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_bitor",  &[i64_type, i64_type], &[i64_type]);
        add_func("ts_bitand", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_bitxor", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_shl",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_shr",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_ushr",   &[i64_type, i64_type], &[i64_type]);
        add_func("ts_bitnot", &[i64_type], &[i64_type]);
        add_func("ts_lt",  &[i64_type, i64_type], &[i32_type]);
        add_func("ts_le",  &[i64_type, i64_type], &[i32_type]);
        add_func("ts_gt",  &[i64_type, i64_type], &[i32_type]);
        add_func("ts_ge",  &[i64_type, i64_type], &[i32_type]);

        add_func("ts_promise_resolve", &[i64_type], &[i64_type]);
        add_func("ts_promise_new",     &[i64_type], &[i64_type]);
        add_func("ts_promise_await",   &[i64_type], &[i64_type]);

        add_func("ts_throw",            &[i64_type], &[]);
        add_func("ts_check_exception",  &[], &[i32_type]);
        add_func("ts_catch_exception",  &[], &[i64_type]);

        add_func("ts_sleep",           &[i32_type], &[i64_type]);
        add_func("ts_promise_race",     &[i64_type, i64_type], &[i64_type]);
        add_func("ts_promise_race_all", &[i64_type], &[i64_type]);
        add_func("ts_promise_then",     &[i64_type, i64_type], &[i64_type]);
        add_func("ts_promise_catch",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_promise_finally",  &[i64_type, i64_type], &[i64_type]);
        add_func("ts_get_promise_constructor", &[], &[i64_type]);
        add_func("ts_get_buffer_constructor",  &[], &[i64_type]);
        add_func("ts_set_timeout",     &[i64_type, i64_type], &[i64_type]);
        add_func("ts_set_interval",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_clear_timeout",   &[i64_type], &[i64_type]);
        add_func("ts_clear_interval",  &[i64_type], &[i64_type]);

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
        add_func("ts_arr_push",        &[i64_type, i64_type], &[]);
        add_func("ts_arr_pop",         &[i64_type], &[i64_type]);
        add_func("ts_arr_unshift",     &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_shift",       &[i64_type], &[i64_type]);
        add_func("ts_arr_push_all",    &[i64_type, i64_type], &[]);
        add_func("ts_arr_join",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_index_of",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_val_index_of",       &[i64_type, i64_type], &[i64_type]);
        add_func("ts_val_includes",       &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_index_of",       &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_index_of_from",  &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_str_last_index_of",  &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_includes",       &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_slice",       &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_str_substring",   &[i64_type, i64_type, i64_type], &[i64_type]);
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
        add_func("ts_math_hypot",  &[i64_type, i64_type], &[i64_type]);
        add_func("ts_math_sign",   &[i64_type], &[i64_type]);
        add_func("ts_math_asin",   &[i64_type], &[i64_type]);
        add_func("ts_math_acos",   &[i64_type], &[i64_type]);
        add_func("ts_math_atan",   &[i64_type], &[i64_type]);
        add_func("ts_math_sinh",   &[i64_type], &[i64_type]);
        add_func("ts_math_cosh",   &[i64_type], &[i64_type]);
        add_func("ts_math_tanh",   &[i64_type], &[i64_type]);
        add_func("ts_math_exp",    &[i64_type], &[i64_type]);
        add_func("ts_math_expm1",  &[i64_type], &[i64_type]);
        add_func("ts_math_log1p",  &[i64_type], &[i64_type]);
        add_func("ts_math_cbrt",   &[i64_type], &[i64_type]);
        add_func("ts_math_clz32",  &[i64_type], &[i64_type]);
        add_func("ts_math_fround", &[i64_type], &[i64_type]);
        add_func("ts_math_random", &[], &[i64_type]);
        add_func("ts_math_imul",   &[i64_type, i64_type], &[i64_type]);
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
        add_func("ts_func_new_this",   &[ptr_type, i32_type], &[i64_type]);
        add_func("ts_func_bind",       &[i64_type, i64_type], &[i64_type]);
        add_func("ts_closure_new",      &[ptr_type, i32_type, i64_type], &[i64_type]);
        add_func("ts_closure_new_rest", &[ptr_type, i32_type, i64_type], &[i64_type]);
        add_func("ts_func_call0",      &[i64_type], &[i64_type]);
        add_func("ts_func_call1",      &[i64_type, i64_type], &[i64_type]);
        add_func("ts_func_call2",      &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_func_call3",      &[i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_func_call4",      &[i64_type, i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_method_call0",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_method_call1",    &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_method_call2",    &[i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_method_call3",    &[i64_type, i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_method_call4",    &[i64_type, i64_type, i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_method_call5",    &[i64_type, i64_type, i64_type, i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_method_call6",    &[i64_type, i64_type, i64_type, i64_type, i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_method_call7",    &[i64_type, i64_type, i64_type, i64_type, i64_type, i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_method_call8",    &[i64_type, i64_type, i64_type, i64_type, i64_type, i64_type, i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_from",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_map",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_filter",      &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_for_each",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_reduce",       &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_reduce_right", &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_find",            &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_find_index",      &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_find_last",       &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_find_last_index", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_some",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_every",       &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_sort",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_flat_map",     &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_flat",         &[i64_type, i32_type], &[i64_type]);
        add_func("ts_arr_concat",       &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_to_sorted",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_to_reversed",  &[i64_type], &[i64_type]);
        add_func("ts_arr_with",         &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_str_search",       &[i64_type, i64_type], &[i64_type]);

        // v1.4: Map built-in
        add_func("ts_map_new",          &[], &[i64_type]);
        add_func("ts_map_from_arr",     &[i64_type], &[i64_type]);
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
        add_func("ts_str_at",             &[i64_type, i64_type], &[i64_type]);
        add_func("ts_val_at",             &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_locale_compare", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_num_to_fixed",       &[i64_type, i64_type], &[i64_type]);
        add_func("ts_num_to_precision",   &[i64_type, i64_type], &[i64_type]);
        add_func("ts_num_to_exponential", &[i64_type, i64_type], &[i64_type]);

        // v1.5: generic computed member get, destructuring rest, Map.entries
        add_func("ts_val_get_key", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_rest",    &[i64_type, i32_type], &[i64_type]);
        add_func("ts_obj_rest",    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_map_entries", &[i64_type], &[i64_type]);

        add_func("ts_json_stringify",  &[i64_type], &[i64_type]);
        add_func("ts_json_parse",      &[i64_type], &[i64_type]);
        add_func("ts_coerce_number",   &[i64_type], &[i64_type]);
        add_func("ts_coerce_string",   &[i64_type], &[i64_type]);
        add_func("ts_func_spread_call",       &[i64_type, i64_type],             &[i64_type]);
        add_func("ts_method_spread_call",     &[i64_type, i64_type, i64_type],   &[i64_type]);
        add_func("ts_encode_uri_component",&[i64_type], &[i64_type]);
        add_func("ts_decode_uri_component",&[i64_type], &[i64_type]);
        add_func("ts_encode_uri",          &[i64_type], &[i64_type]);
        add_func("ts_decode_uri",          &[i64_type], &[i64_type]);

        add_func("ts_regexp_new",          &[ptr_type, ptr_type], &[i64_type]);
        add_func("ts_regexp_from_val",     &[i64_type, i64_type], &[i64_type]);
        add_func("ts_regexp_test",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_regexp_exec",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_match",           &[i64_type, i64_type], &[i64_type]);
        add_func("ts_str_match_all",       &[i64_type, i64_type], &[i64_type]);
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
        add_func("ts_headers_get",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_headers_has",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_headers_set",         &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_headers_delete",      &[i64_type, i64_type], &[i64_type]);
        add_func("ts_response_new",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_response_clone",      &[i64_type], &[i64_type]);
        add_func("ts_response_status",     &[i64_type], &[i64_type]);
        add_func("ts_response_ok",         &[i64_type], &[i64_type]);
        add_func("ts_response_headers",    &[i64_type], &[i64_type]);
        add_func("ts_request_new",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_fetch",               &[i64_type, i64_type], &[i64_type]);

        // Module globals (cross-function shared state)
        add_func("ts_set_module_global",   &[ptr_type, i64_type], &[]);
        add_func("ts_get_module_global",   &[ptr_type], &[i64_type]);
        add_func("ts_process_exit",        &[i32_type], &[]);
        add_func("ts_process_argv",        &[], &[i64_type]);
        add_func("ts_process_env",         &[], &[i64_type]);
        add_func("ts_process_pid",         &[], &[i64_type]);
        add_func("ts_import_meta_new",     &[], &[i64_type]);

        // Additional builtins
        add_func("ts_promise_reject",       &[i64_type], &[i64_type]);
        add_func("ts_promise_all",         &[i64_type], &[i64_type]);
        add_func("ts_promise_all_settled", &[i64_type], &[i64_type]);
        add_func("ts_promise_any",         &[i64_type], &[i64_type]);
        add_func("ts_val_has_key",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_coerce_bool",         &[i64_type], &[i64_type]);

        // HTTP server
        add_func("ts_serve",               &[i32_type, i64_type], &[i64_type]);

        // Closure introspection (for recursive inner functions)
        add_func("ts_closure_get_env",     &[i64_type], &[i64_type]);

        // URL / URLSearchParams
        add_func("ts_url_new",                    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_urlsearchparams_new",        &[i64_type], &[i64_type]);
        add_func("ts_urlsearchparams_to_string",  &[i64_type], &[i64_type]);
        add_func("ts_urlsearchparams_append",     &[i64_type, i64_type, i64_type], &[]);
        add_func("ts_urlsearchparams_get_all",    &[i64_type, i64_type], &[i64_type]);

        // Request/Response body methods
        add_func("ts_val_text",                   &[i64_type], &[i64_type]);
        add_func("ts_val_json",                   &[i64_type], &[i64_type]);

        // addEventListener / serve(port) with registered listener
        add_func("ts_add_event_listener",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_remove_event_listener",      &[i64_type, i64_type], &[i64_type]);
        add_func("ts_serve_worker",               &[i32_type], &[i64_type]);

        // Heap profiler init (no-op unless ts-runtime built with --features dhat-heap)
        add_func("ts_dhat_init",                  &[], &[]);

        // Symbol
        add_func("ts_symbol_new",                 &[i64_type], &[i64_type]);
        add_func("ts_symbol_description",         &[i64_type], &[i64_type]);

        // Set
        add_func("ts_set_new",                    &[], &[i64_type]);
        add_func("ts_set_new_from_iter",          &[i64_type], &[i64_type]);
        add_func("ts_set_add",                    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_set_has",                    &[i64_type, i64_type], &[i64_type]);
        add_func("ts_set_delete",                 &[i64_type, i64_type], &[i64_type]);
        add_func("ts_set_clear",                  &[i64_type], &[]);
        add_func("ts_set_size",                   &[i64_type], &[i64_type]);
        add_func("ts_set_keys",                   &[i64_type], &[i64_type]);
        add_func("ts_set_values",                 &[i64_type], &[i64_type]);
        add_func("ts_set_entries",                &[i64_type], &[i64_type]);
        add_func("ts_set_for_each",               &[i64_type, i64_type], &[i64_type]);

        // WeakMap
        add_func("ts_weakmap_new",                &[], &[i64_type]);
        add_func("ts_weakmap_set",                &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_weakmap_get",                &[i64_type, i64_type], &[i64_type]);
        add_func("ts_weakmap_has",                &[i64_type, i64_type], &[i64_type]);
        add_func("ts_weakmap_delete",             &[i64_type, i64_type], &[i64_type]);

        // WeakSet
        add_func("ts_weakset_new",                &[], &[i64_type]);
        add_func("ts_weakset_add",                &[i64_type, i64_type], &[i64_type]);
        add_func("ts_weakset_has",                &[i64_type, i64_type], &[i64_type]);
        add_func("ts_weakset_delete",             &[i64_type, i64_type], &[i64_type]);

        // Reflect metadata API
        add_func("ts_reflect_metadata_decorator", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_apply_decorators",           &[i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_reflect_define_metadata",    &[i64_type, i64_type, i64_type, i64_type], &[]);
        add_func("ts_reflect_get_metadata",       &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_reflect_get_own_metadata",   &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_reflect_has_metadata",       &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_reflect_has_own_metadata",   &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_reflect_get_metadata_keys",  &[i64_type, i64_type], &[i64_type]);
        add_func("ts_reflect_get_own_metadata_keys", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_reflect_delete_metadata",    &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_reflect_get_prototype_of",              &[i64_type], &[i64_type]);
        add_func("ts_reflect_get_own_property_descriptor",   &[i64_type, i64_type], &[i64_type]);

        // Object introspection
        add_func("ts_obj_get_own_property_names", &[i64_type], &[i64_type]);
        add_func("ts_obj_get_prototype_of",       &[i64_type], &[i64_type]);
        add_func("ts_obj_define_property",        &[i64_type, i64_type, i64_type], &[]);
        add_func("ts_obj_define_getter",          &[i64_type, ptr_type, i64_type], &[]);
        add_func("ts_obj_define_setter",          &[i64_type, ptr_type, i64_type], &[]);
        add_func("ts_structured_clone",           &[i64_type], &[i64_type]);
        add_func("ts_queue_microtask",            &[i64_type], &[i64_type]);

        // Polymorphic container dispatch
        add_func("ts_container_get",              &[i64_type, i64_type], &[i64_type]);
        add_func("ts_container_set",              &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_container_add",              &[i64_type, i64_type], &[i64_type]);
        add_func("ts_container_has",              &[i64_type, i64_type], &[i64_type]);
        add_func("ts_container_delete",           &[i64_type, i64_type], &[i64_type]);
        add_func("ts_container_clear",            &[i64_type], &[]);
        add_func("ts_container_size",             &[i64_type], &[i64_type]);
        add_func("ts_container_keys",             &[i64_type], &[i64_type]);
        add_func("ts_container_values",           &[i64_type], &[i64_type]);
        add_func("ts_container_entries",          &[i64_type], &[i64_type]);
        add_func("ts_container_for_each",         &[i64_type, i64_type], &[i64_type]);

        // String trim variants
        add_func("ts_str_trim_start",             &[i64_type], &[i64_type]);
        add_func("ts_str_trim_end",               &[i64_type], &[i64_type]);

        // Array mutating methods
        add_func("ts_arr_reverse",                &[i64_type], &[i64_type]);
        add_func("ts_arr_fill",                   &[i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_splice",                 &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_copy_within",            &[i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_last_index_of",          &[i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_slice_range",            &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_arr_includes",               &[i64_type, i64_type], &[i64_type]);

        // Number static methods
        add_func("ts_number_is_integer",          &[i64_type], &[i64_type]);
        add_func("ts_number_is_finite",           &[i64_type], &[i64_type]);
        add_func("ts_number_is_nan",              &[i64_type], &[i64_type]);
        add_func("ts_number_is_safe_integer",     &[i64_type], &[i64_type]);

        // Date
        add_func("ts_date_new",                   &[], &[i64_type]);
        add_func("ts_date_from_val",              &[i64_type], &[i64_type]);
        add_func("ts_date_now",                   &[], &[i64_type]);
        add_func("ts_date_get_time",              &[i64_type], &[i64_type]);
        add_func("ts_date_get_full_year",         &[i64_type], &[i64_type]);
        add_func("ts_date_get_month",             &[i64_type], &[i64_type]);
        add_func("ts_date_get_date",              &[i64_type], &[i64_type]);
        add_func("ts_date_get_day",               &[i64_type], &[i64_type]);
        add_func("ts_date_get_hours",             &[i64_type], &[i64_type]);
        add_func("ts_date_get_minutes",           &[i64_type], &[i64_type]);
        add_func("ts_date_get_seconds",           &[i64_type], &[i64_type]);
        add_func("ts_date_get_milliseconds",      &[i64_type], &[i64_type]);
        add_func("ts_date_to_iso_string",         &[i64_type], &[i64_type]);
        add_func("ts_date_to_locale_date_string", &[i64_type], &[i64_type]);
        add_func("ts_date_to_locale_time_string", &[i64_type], &[i64_type]);
        add_func("ts_date_to_string",             &[i64_type], &[i64_type]);
        add_func("ts_weakref_new",                &[i64_type], &[i64_type]);
        add_func("ts_weakref_deref",              &[i64_type], &[i64_type]);

        // Symbol well-known values + iterables
        add_func("ts_symbol_iterator",            &[], &[i64_type]);
        add_func("ts_normalize_iterable",         &[i64_type], &[i64_type]);

        // Node.js path module
        add_func("ts_path_join",                  &[i64_type], &[i64_type]);
        add_func("ts_path_resolve",               &[i64_type], &[i64_type]);
        add_func("ts_path_dirname",               &[i64_type], &[i64_type]);
        add_func("ts_path_basename",              &[i64_type, i64_type], &[i64_type]);
        add_func("ts_path_extname",               &[i64_type], &[i64_type]);
        add_func("ts_path_normalize",             &[i64_type], &[i64_type]);
        add_func("ts_path_is_absolute",           &[i64_type], &[i64_type]);
        add_func("ts_path_relative",              &[i64_type, i64_type], &[i64_type]);

        // Node.js os module
        add_func("ts_os_platform",                &[], &[i64_type]);
        add_func("ts_os_homedir",                 &[], &[i64_type]);
        add_func("ts_os_tmpdir",                  &[], &[i64_type]);
        add_func("ts_os_hostname",                &[], &[i64_type]);
        add_func("ts_os_eol",                     &[], &[i64_type]);
        add_func("ts_os_arch",                    &[], &[i64_type]);
        add_func("ts_os_cpus",                    &[], &[i64_type]);

        // Node.js fs module
        add_func("ts_fs_read_file_sync",          &[i64_type, i64_type], &[i64_type]);
        add_func("ts_fs_write_file_sync",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_fs_exists_sync",             &[i64_type], &[i64_type]);
        add_func("ts_fs_mkdir_sync",              &[i64_type, i64_type], &[i64_type]);
        add_func("ts_fs_readdir_sync",            &[i64_type], &[i64_type]);
        add_func("ts_fs_stat_sync",               &[i64_type], &[i64_type]);
        add_func("ts_fs_unlink_sync",             &[i64_type], &[i64_type]);
        add_func("ts_fs_rename_sync",             &[i64_type, i64_type], &[i64_type]);
        add_func("ts_fs_copy_file_sync",          &[i64_type, i64_type], &[i64_type]);
        add_func("ts_fs_rm_sync",                 &[i64_type, i64_type], &[i64_type]);
        add_func("ts_fs_read_file_async",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_fs_write_file_async",        &[i64_type, i64_type], &[i64_type]);

        // Node.js crypto module
        add_func("ts_crypto_random_uuid",         &[], &[i64_type]);
        add_func("ts_crypto_random_bytes_hex",    &[i64_type], &[i64_type]);
        add_func("ts_crypto_random_bytes",        &[i64_type], &[i64_type]);
        add_func("ts_crypto_hash_sync",           &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_crypto_hmac_sync",           &[i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_crypto_pbkdf2_sync",         &[i64_type, i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_crypto_scrypt_sync",         &[i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_crypto_timing_safe_equal",   &[i64_type, i64_type], &[i64_type]);
        add_func("ts_crypto_random_fill_sync",    &[i64_type], &[i64_type]);

        // Node.js events module (EventEmitter)
        add_func("ts_event_emitter_new",          &[], &[i64_type]);
        add_func("ts_event_emitter_on",           &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_event_emitter_once",         &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_event_emitter_off",          &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_event_emitter_emit",         &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_event_emitter_remove_all_listeners", &[i64_type, i64_type], &[i64_type]);
        add_func("ts_event_emitter_listeners",    &[i64_type, i64_type], &[i64_type]);

        // Node.js buffer module
        add_func("ts_buffer_from_string",         &[i64_type, i64_type], &[i64_type]);
        add_func("ts_buffer_from_array",          &[i64_type], &[i64_type]);
        add_func("ts_buffer_alloc",               &[i64_type, i64_type], &[i64_type]);
        add_func("ts_buffer_alloc_unsafe",        &[i64_type], &[i64_type]);
        add_func("ts_buffer_concat",              &[i64_type, i64_type], &[i64_type]);
        add_func("ts_buffer_to_string",           &[i64_type, i64_type], &[i64_type]);
        add_func("ts_buffer_to_string_range",     &[i64_type, i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_buffer_length",              &[i64_type], &[i64_type]);
        add_func("ts_buffer_slice",               &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_buffer_get_byte",            &[i64_type, i64_type], &[i64_type]);

        // Node.js process extensions
        add_func("ts_process_cwd",                &[], &[i64_type]);
        add_func("ts_process_platform",           &[], &[i64_type]);
        add_func("ts_process_version",            &[], &[i64_type]);
        add_func("ts_process_versions",           &[], &[i64_type]);
        add_func("ts_process_hrtime",             &[], &[i64_type]);
        add_func("ts_process_uptime",             &[], &[i64_type]);

        // performance built-in
        add_func("ts_performance_now",            &[], &[i64_type]);
        add_func("ts_performance_mark",           &[i64_type], &[i64_type]);
        add_func("ts_performance_measure",        &[i64_type, i64_type], &[i64_type]);
        add_func("ts_performance_get_entries_by_name", &[i64_type], &[i64_type]);

        // Node.js dns module
        add_func("ts_dns_lookup",                 &[i64_type], &[i64_type]);
        add_func("ts_dns_resolve",                &[i64_type, i64_type], &[i64_type]);
        add_func("ts_dns_lookup_async",           &[i64_type], &[i64_type]);
        add_func("ts_dns_resolve_async",          &[i64_type, i64_type], &[i64_type]);

        // Node.js http module
        add_func("ts_http_server_listen",         &[i64_type, i64_type], &[i64_type]);

        // Node.js net module
        add_func("ts_net_server_listen",          &[i64_type, i64_type], &[i64_type]);
        add_func("ts_net_connect",                &[i64_type, i64_type, i64_type], &[i64_type]);

        // Node.js url module
        add_func("ts_url_parse",                  &[i64_type, i64_type], &[i64_type]);
        add_func("ts_url_resolve",                &[i64_type, i64_type], &[i64_type]);
        add_func("ts_url_format",                 &[i64_type], &[i64_type]);

        // Node.js child_process module
        add_func("ts_exec_sync",                  &[i64_type], &[i64_type]);
        add_func("ts_spawn_sync",                 &[i64_type, i64_type, i64_type], &[i64_type]);
        add_func("ts_exec_async",                 &[i64_type], &[i64_type]);

        // Node.js zlib module
        add_func("ts_zlib_deflate_sync",          &[i64_type], &[i64_type]);
        add_func("ts_zlib_inflate_sync",          &[i64_type], &[i64_type]);
        add_func("ts_zlib_gzip_sync",             &[i64_type], &[i64_type]);
        add_func("ts_zlib_gunzip_sync",           &[i64_type], &[i64_type]);
        add_func("ts_zlib_deflate_async",         &[i64_type], &[i64_type]);
        add_func("ts_zlib_inflate_async",         &[i64_type], &[i64_type]);
        add_func("ts_zlib_gzip_async",            &[i64_type], &[i64_type]);
        add_func("ts_zlib_gunzip_async",          &[i64_type], &[i64_type]);

        // Node.js readline module
        add_func("ts_readline_question",          &[i64_type], &[i64_type]);
        add_func("ts_readline_read_line",         &[], &[i64_type]);

        // CJS module namespace registry
        add_func("ts_cjs_register_ns",  &[i64_type, i64_type], &[]);
        add_func("ts_cjs_require_ns",   &[i64_type], &[i64_type]);

        // Node-API export registration (used in --emit-node-addon mode).
        add_func("ts_napi_register_export", &[ptr_type, ptr_type, i32_type], &[]);

        // Mark all runtime-declared functions as emitted so that `declare function`
        // in shim files doesn't try to re-declare them (which causes MLIR symbol redefinition).
        drop(add_func);
        for name in declared_names {
            self.emitted_functions.insert(name);
        }
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
                    // `declare function` with no body: register as extern FFI with the declared arity.
                    if func.declare && func.body.is_none() {
                        let name = id.name.to_string();
                        if !self.funcs.contains_key(&name) {
                            let n = func.params.items.len();
                            self.funcs.insert(name, FuncSig {
                                param_types: vec![i64_type; n],
                                return_type: Some(i64_type),
                                has_rest: false,
                                has_this_param: false,
                            });
                        }
                        continue;
                    }
                    // Skip TypeScript overload signatures (declarations without a body) — they would
                    // shadow the actual implementation's signature with a wrong arity.
                    if func.body.is_none() { continue; }
                    let raw = id.name.to_string();
                    let name = if raw == "main" {
                        // "main" conflicts with LLVM entry point — rename and add alias so call sites resolve.
                        self.module_global_aliases.insert("main".to_string(), "__user_main".to_string());
                        "__user_main".to_string()
                    } else { raw };
                    // First-wins: if a function with this name was already registered (from an
                    // earlier imported module), keep the existing signature so it stays consistent
                    // with the first-emitted body (see emitted_functions in lower_function_declaration).
                    if self.funcs.contains_key(&name) { continue; }
                    let explicit_rest = func.params.rest.is_some();
                    let implicit_rest = !explicit_rest && func.body.as_ref().map_or(false, |b| {
                        crate::lowering::expressions::body_uses_arguments(&b.statements)
                    });
                    let has_rest = explicit_rest || implicit_rest;
                    let has_this_param = func.this_param.is_some();
                    let n = func.params.items.len()
                        + if has_rest { 1 } else { 0 }
                        + if has_this_param { 1 } else { 0 };
                    self.funcs.insert(name, FuncSig {
                        param_types: vec![i64_type; n],
                        return_type: Some(i64_type),
                        has_rest,
                        has_this_param,
                    });
                }
                Statement::ExportNamedDeclaration(export) => {
                    if let Some(Declaration::FunctionDeclaration(func)) = &export.declaration {
                        if func.body.is_none() { continue; } // Skip overload signatures
                        if let Some(id) = &func.id {
                            let name = id.name.to_string();
                            if self.funcs.contains_key(&name) { continue; }
                            let explicit_rest = func.params.rest.is_some();
                            let implicit_rest = !explicit_rest && func.body.as_ref().map_or(false, |b| {
                                crate::lowering::expressions::body_uses_arguments(&b.statements)
                            });
                            let has_rest = explicit_rest || implicit_rest;
                            let has_this_param = func.this_param.is_some();
                            let n = func.params.items.len()
                                + if has_rest { 1 } else { 0 }
                                + if has_this_param { 1 } else { 0 };
                            self.funcs.insert(name, FuncSig {
                                param_types: vec![i64_type; n],
                                return_type: Some(i64_type),
                                has_rest,
                                has_this_param,
                            });
                        }
                    }
                    if let Some(Declaration::VariableDeclaration(vd)) = &export.declaration {
                        self.collect_const_sigs(vd, i64_type);
                    }
                    // Exported decorated classes are stored as module globals at runtime.
                    if let Some(Declaration::ClassDeclaration(class)) = &export.declaration {
                        if !class.decorators.is_empty() {
                            if let Some(id) = &class.id {
                                self.module_global_names.insert(id.name.to_string());
                            }
                        }
                    }
                }
                Statement::ExportDefaultDeclaration(export) => {
                    use oxc_ast::ast::ExportDefaultDeclarationKind;
                    if let ExportDefaultDeclarationKind::FunctionDeclaration(func) = &export.declaration {
                        if func.body.is_none() { continue; } // Skip overload signatures
                        if let Some(id) = &func.id {
                            let name = id.name.to_string();
                            if self.funcs.contains_key(&name) { continue; }
                            let explicit_rest = func.params.rest.is_some();
                            let implicit_rest = !explicit_rest && func.body.as_ref().map_or(false, |b| {
                                crate::lowering::expressions::body_uses_arguments(&b.statements)
                            });
                            let has_rest = explicit_rest || implicit_rest;
                            let has_this_param = func.this_param.is_some();
                            let n = func.params.items.len()
                                + if has_rest { 1 } else { 0 }
                                + if has_this_param { 1 } else { 0 };
                            self.funcs.insert(name, FuncSig {
                                param_types: vec![i64_type; n],
                                return_type: Some(i64_type),
                                has_rest,
                                has_this_param,
                            });
                        }
                    }
                }
                // Module-level `const name = arrow` — hoist as function.
                // Module-level `const name = identifier` — track as alias.
                Statement::VariableDeclaration(vd) => {
                    self.collect_const_sigs(vd, i64_type);
                }
                // Decorated class declarations are stored as module globals at runtime
                // so that top-level function declarations can access them via ts_get_module_global.
                Statement::ClassDeclaration(class) => {
                    if !class.decorators.is_empty() {
                        if let Some(id) = &class.id {
                            self.module_global_names.insert(id.name.to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Restricted signature collection for CJS modules: only registers top-level `function`
    /// declarations (not const-function-expressions) to avoid polluting the global namespace.
    pub(super) fn collect_cjs_function_signatures(&mut self, program: &Program<'_>) {
        let i64_type = self.i64_type();
        for stmt in &program.body {
            if let Statement::FunctionDeclaration(func) = stmt {
                let Some(id) = &func.id else { continue };
                if func.body.is_none() { continue; } // skip TS overloads
                let name = id.name.to_string();
                if self.funcs.contains_key(&name) { continue; } // first-wins
                let explicit_rest = func.params.rest.is_some();
                let implicit_rest = !explicit_rest && func.body.as_ref().map_or(false, |b| {
                    crate::lowering::expressions::body_uses_arguments(&b.statements)
                });
                let has_rest = explicit_rest || implicit_rest;
                let has_this_param = func.this_param.is_some();
                let n = func.params.items.len()
                    + if has_rest { 1 } else { 0 }
                    + if has_this_param { 1 } else { 0 };
                self.funcs.insert(name, FuncSig {
                    param_types: vec![i64_type; n],
                    return_type: Some(i64_type),
                    has_rest,
                    has_this_param,
                });
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
                    // First-wins: don't overwrite a signature already registered by a
                    // FunctionDeclaration (e.g., from a public API of the same name).
                    if self.funcs.contains_key(&name) { continue; }
                    let has_rest = arrow.params.rest.is_some();
                    let n = arrow.params.items.len() + if has_rest { 1 } else { 0 };
                    self.funcs.insert(name, FuncSig {
                        param_types: vec![i64_type; n],
                        return_type: Some(i64_type),
                        has_rest,
                        has_this_param: false,
                    });
                }
                Expression::FunctionExpression(func_expr) => {
                    // First-wins: same policy as FunctionDeclaration.
                    if self.funcs.contains_key(&name) { continue; }
                    let has_rest = func_expr.params.rest.is_some();
                    let has_this_param = func_expr.this_param.is_some();
                    let n = func_expr.params.items.len()
                        + if has_rest { 1 } else { 0 }
                        + if has_this_param { 1 } else { 0 };
                    self.funcs.insert(name, FuncSig {
                        param_types: vec![i64_type; n],
                        return_type: Some(i64_type),
                        has_rest,
                        has_this_param,
                    });
                }
                Expression::Identifier(id) => {
                    // `const alias = someFunc` — record as alias for call dispatch.
                    self.builtin_aliases.insert(name, id.name.to_string());
                }
                Expression::ClassExpression(class_expr) => {
                    // `const Foo = class ...` — register constructor signature
                    let ctor = class_expr.body.body.iter().find_map(|elem| {
                        use oxc_ast::ast::{ClassElement, MethodDefinitionKind};
                        if let ClassElement::MethodDefinition(m) = elem {
                            if m.kind == MethodDefinitionKind::Constructor { Some(m) } else { None }
                        } else { None }
                    });
                    let n = ctor.map(|c| c.value.params.items.len()).unwrap_or(0);
                    let ctor_name = format!("__class_{}_constructor", name);
                    self.funcs.insert(ctor_name, FuncSig {
                        param_types: vec![i64_type; 1 + n], // +1 for self
                        return_type: Some(i64_type),
                        has_rest: false,
                        has_this_param: false,
                    });
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
            let mut method_arity: HashMap<String, usize> = HashMap::new();
            let mut method_has_rest: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut statics: HashMap<String, String> = HashMap::new();
            let mut getters: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut setters: std::collections::HashSet<String> = std::collections::HashSet::new();

            // Collect constructor arity (number of MLIR params the constructor will take).
            let builtin_error_names = ["Error", "TypeError", "RangeError", "ReferenceError", "SyntaxError"];
            let parent_name_for_arity = class.super_class.as_ref().and_then(|e| {
                if let Expression::Identifier(id) = e { Some(id.name.as_str()) } else { None }
            });
            let ctor_elem = class.body.body.iter().find(|e| {
                matches!(e, ClassElement::MethodDefinition(m) if m.kind == MethodDefinitionKind::Constructor && m.value.body.is_some())
            });
            let constructor_arity = match ctor_elem {
                Some(ClassElement::MethodDefinition(m)) => m.value.params.items.len(),
                _ => {
                    // No explicit constructor: implicit error param if parent is builtin Error.
                    if parent_name_for_arity.map(|n| builtin_error_names.contains(&n)).unwrap_or(false) { 1 } else { 0 }
                }
            };

            // Collect field_class_types: constructor parameter properties with class type annotations.
            // e.g. `constructor(private repo: Repo)` → field_class_types["repo"] = "Repo"
            let mut field_class_types: HashMap<String, String> = HashMap::new();
            if let Some(ClassElement::MethodDefinition(ctor)) = ctor_elem {
                for param in &ctor.value.params.items {
                    // Only parameter properties (private/public/protected/readonly)
                    if param.accessibility.is_none() && !param.readonly { continue; }
                    let field_name = match &param.pattern {
                        BindingPattern::BindingIdentifier(id) => id.name.to_string(),
                        _ => continue,
                    };
                    // Extract the class name from TSTypeAnnotation → TSTypeReference
                    if let Some(ann) = &param.type_annotation {
                        if let oxc_ast::ast::TSType::TSTypeReference(tref) = &ann.type_annotation {
                            if let oxc_ast::ast::TSTypeName::IdentifierReference(id) = &tref.type_name {
                                let type_name = id.name.to_string();
                                // Only record if it's a known user class (not a primitive/builtin)
                                let primitives = ["string", "number", "boolean", "any", "void",
                                                  "never", "unknown", "object", "null", "undefined",
                                                  "String", "Number", "Boolean", "Function",
                                                  "Array", "Map", "Set", "Promise", "Date"];
                                if !primitives.contains(&type_name.as_str()) {
                                    field_class_types.insert(field_name, type_name);
                                }
                            }
                        }
                    }
                }
            }

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
                        // arity = 1 (self) + positional params + (1 if rest param)
                        let has_rest = method.value.params.rest.is_some();
                        let arity = 1 + method.value.params.items.len() + if has_rest { 1 } else { 0 };
                        method_arity.insert(name.clone(), arity);
                        if has_rest { method_has_rest.insert(name.clone()); }
                        methods.insert(name, mangled);
                    }
                    _ => {}
                }
            }

            let parent = class.super_class.as_ref().and_then(|e| {
                if let Expression::Identifier(id) = e { Some(id.name.to_string()) } else { None }
            });

            // Collect static property field names.
            let mut static_fields: std::collections::HashSet<String> = std::collections::HashSet::new();
            for element in &class.body.body {
                use oxc_ast::ast::ClassElement;
                if let ClassElement::PropertyDefinition(prop) = element {
                    if prop.r#static {
                        if let Some(name) = prop.key.static_name() {
                            static_fields.insert(name.to_string());
                        }
                    }
                }
            }

            own_members.push((class_name.clone(), ClassSig {
                constructor_name: format!("__class_{}_constructor", class_name),
                constructor_arity,
                methods,
                method_arity,
                method_has_rest,
                statics,
                getters,
                setters,
                static_fields,
                field_class_types,
                parent,
            }));
        }

        // Pass 2: insert in order; inherit from parent (already inserted if declared first).
        // First-wins: skip classes already registered (from an earlier imported module) to keep
        // class signatures consistent with the first-emitted body (see lowered_classes).
        for (class_name, mut sig) in own_members {
            if self.classes.contains_key(&class_name) { continue; }
            if let Some(parent_name) = sig.parent.clone() {
                if let Some(parent_sig) = self.classes.get(&parent_name).cloned() {
                    for (n, m) in &parent_sig.methods {
                        sig.methods.entry(n.clone()).or_insert_with(|| m.clone());
                    }
                    for (n, a) in &parent_sig.method_arity {
                        sig.method_arity.entry(n.clone()).or_insert(*a);
                    }
                    for n in &parent_sig.method_has_rest {
                        sig.method_has_rest.insert(n.clone());
                    }
                    for n in &parent_sig.getters {
                        if !sig.getters.contains(n) { sig.getters.insert(n.clone()); }
                    }
                    for n in &parent_sig.setters {
                        if !sig.setters.contains(n) { sig.setters.insert(n.clone()); }
                    }
                    for (n, t) in &parent_sig.field_class_types {
                        sig.field_class_types.entry(n.clone()).or_insert_with(|| t.clone());
                    }
                }
            }
            self.classes.insert(class_name, sig);
        }
    }

    // ── Function declarations ─────────────────────────────────────────────

    pub fn lower_function_declaration_as(&mut self, func: &Function<'_>, emit_name: &str) -> Result<()> {
        self.lower_function_declaration_impl(func, Some(emit_name))
    }

    /// Lower a `FunctionExpression` as a named top-level MLIR function.
    /// Used when lowering CJS exports: `exports.foo = function(...) { ... }` → `@foo`.
    pub fn lower_arrow_or_func_expr_as(&mut self, func: &oxc_ast::ast::Function<'_>, emit_name: &str) -> Result<()> {
        // Skip if already emitted under this name
        if self.emitted_functions.contains(emit_name) {
            return Ok(());
        }
        let i64_type = self.i64_type();
        let return_type = i64_type;
        let explicit_rest = func.params.rest.is_some();
        let implicit_rest = !explicit_rest && func.body.as_ref().map_or(false, |b| {
            crate::lowering::expressions::body_uses_arguments(&b.statements)
        });
        let has_rest = explicit_rest || implicit_rest;
        let has_this_param = func.this_param.is_some();
        let this_offset: usize = if has_this_param { 1 } else { 0 };
        let n_params = func.params.items.len()
            + if has_rest { 1 } else { 0 }
            + this_offset;
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..n_params).map(|_| (i64_type, self.loc)).collect();
        let func_type = FunctionType::new(self.ctx, &vec![i64_type; n_params], &[return_type]);
        let region = Region::new();
        let entry = region.append_block(Block::new(&param_specs));
        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        let mut param_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        if has_this_param {
            let this_val: Value<'_, '_> = entry.argument(0)?.into();
            scope.insert("this".to_string(), this_val);
            param_names.insert("this".to_string());
        }
        let mut current_block = entry;
        for (i, param) in func.params.items.iter().enumerate() {
            let arg_val: Value<'_, '_> = entry.argument(i + this_offset)?.into();
            match &param.pattern {
                BindingPattern::BindingIdentifier(id) => {
                    let pname = id.name.to_string();
                    param_names.insert(pname.clone());
                    scope.insert(pname, arg_val);
                }
                BindingPattern::ArrayPattern(_) | BindingPattern::ObjectPattern(_) => {
                    let arg_i64 = self.ensure_i64(arg_val, current_block)?;
                    current_block = self.lower_bind_pattern(&param.pattern, arg_i64, current_block, &region, &mut scope)?;
                }
                _ => {}
            }
        }
        if let Some(rest) = &func.params.rest {
            let rest_idx = func.params.items.len() + this_offset;
            if let BindingPattern::BindingIdentifier(id) = &rest.rest.argument {
                let rname = id.name.to_string();
                param_names.insert(rname.clone());
                scope.insert(rname, entry.argument(rest_idx)?.into());
            }
        }
        // Inject module globals into scope
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

        let saved_fn_return_type = self.fn_return_type;
        let saved_is_async = self.is_async;
        let saved_fn_params = std::mem::replace(&mut self.current_fn_params, param_names.clone());
        let saved_cell_vars = std::mem::replace(&mut self.cell_vars, std::collections::HashSet::new());
        let saved_cell_captures = std::mem::replace(&mut self.cell_captures, std::collections::HashSet::new());
        // scalar_vars / non_escaping_allocs will be populated after we know the body; save empty for now
        let saved_scalar_vars = std::mem::replace(&mut self.scalar_vars, std::collections::HashSet::new());
        let saved_non_escaping = std::mem::replace(&mut self.non_escaping_allocs, std::collections::HashSet::new());
        // (populated below once body is confirmed)
        self.fn_return_type = return_type;
        self.is_async = func.r#async;

        let Some(body) = &func.body else {
            // No body — emit a stub returning undefined
            let undef_val: Value<'_, '_> = entry.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            entry.append_operation(func::r#return(&[undef_val], self.loc));
            let fn_op = func::func(
                self.ctx,
                StringAttribute::new(self.ctx, emit_name),
                TypeAttribute::new(func_type.into()),
                region, &[(
                    Identifier::new(self.ctx, "sym_visibility"),
                    StringAttribute::new(self.ctx, "private").into(),
                )], self.loc,
            );
            self.module.body().append_operation(fn_op);
            self.fn_return_type = saved_fn_return_type;
            self.is_async = saved_is_async;
            self.current_fn_params = saved_fn_params;
            self.cell_vars = saved_cell_vars;
            self.cell_captures = saved_cell_captures;
            self.scalar_vars = saved_scalar_vars;
            self.non_escaping_allocs = saved_non_escaping;
            self.funcs.insert(emit_name.to_string(), FuncSig {
                param_types: vec![i64_type; n_params],
                return_type: Some(return_type),
                has_rest,
                has_this_param,
            });
            self.emitted_functions.insert(emit_name.to_string());
            return Ok(());
        };

        // Compute cell vars for mutable captures, then scalar vars (excluding cell vars),
        // then non-escaping allocs (excluding cell vars and scalar vars).
        let cell_vars_set = crate::lowering::expressions::compute_cell_vars_for_body(&body.statements);
        self.cell_vars = cell_vars_set;
        let mut sv = crate::lowering::expressions::compute_scalar_vars_for_body(&body.statements);
        sv.retain(|v| !self.cell_vars.contains(v));
        let mut nea = crate::lowering::expressions::compute_non_escaping_allocs(&body.statements);
        nea.retain(|v| !self.cell_vars.contains(v));
        self.non_escaping_allocs = nea;
        // Parameters with scalar TypeScript type annotations are also scalar.
        for param in &func.params.items {
            if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                if let Some(ann) = &param.type_annotation {
                    if crate::lowering::ts_type_is_scalar(&ann.type_annotation) {
                        sv.insert(id.name.to_string());
                    }
                }
            }
        }
        self.scalar_vars = sv;

        let mut result_val: Value<'_, '_> = entry.append_operation(arith::constant(
            self.ctx, IntegerAttribute::new(return_type, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
        )).result(0)?.into();

        for s in &body.statements {
            let (v_opt, nb) = self.lower_statement(s, current_block, &region, &mut scope, &[])?;
            current_block = nb;
            if let Some(v) = v_opt {
                result_val = self.ensure_i64(v, current_block)?;
            }
        }

        // Release module globals loaded at start
        for gname in self.module_global_names.clone() {
            if let Some(val) = scope.get(&gname) {
                if !param_names.contains(&gname) {
                    let v_i64 = self.ensure_i64(*val, current_block)?;
                    current_block.append_operation(func::call(
                        self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                        &[v_i64], &[], self.loc,
                    ));
                }
            }
        }

        let undef_i64: Value<'_, '_> = current_block.append_operation(arith::constant(
            self.ctx, IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
        )).result(0)?.into();
        self.terminate_with_return(current_block, undef_i64)?;

        let fn_op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, emit_name),
            TypeAttribute::new(func_type.into()),
            region, &[(
                Identifier::new(self.ctx, "sym_visibility"),
                StringAttribute::new(self.ctx, "private").into(),
            )], self.loc,
        );
        self.module.body().append_operation(fn_op);

        self.fn_return_type = saved_fn_return_type;
        self.is_async = saved_is_async;
        self.current_fn_params = saved_fn_params;
        self.cell_vars = saved_cell_vars;
        self.cell_captures = saved_cell_captures;
        self.scalar_vars = saved_scalar_vars;
        self.non_escaping_allocs = saved_non_escaping;
        self.funcs.insert(emit_name.to_string(), FuncSig {
            param_types: vec![i64_type; n_params],
            return_type: Some(return_type),
            has_rest,
            has_this_param,
        });
        self.emitted_functions.insert(emit_name.to_string());
        Ok(())
    }

    pub fn lower_function_declaration(&mut self, func: &Function<'_>) -> Result<()> {
        self.lower_function_declaration_impl(func, None)
    }

    fn lower_function_declaration_impl(&mut self, func: &Function<'_>, name_override: Option<&str>) -> Result<()> {
        let Some(id) = &func.id else { return Ok(()) };
        // `declare function foo(...)` — emit an external function declaration (no body).
        // This enables FFI: users can link native C/Rust libraries and call them from TypeScript.
        // All parameters and the return value are i64 (TsVal / NaN-boxed).
        if func.declare && func.body.is_none() {
            let name = id.name.to_string();
            if !self.emitted_functions.insert(name.clone()) {
                return Ok(());
            }
            let i64_type = self.i64_type();
            let n_params = func.params.items.len();
            let func_type = FunctionType::new(self.ctx, &vec![i64_type; n_params], &[i64_type]);
            let private = &[(
                Identifier::new(self.ctx, "sym_visibility"),
                StringAttribute::new(self.ctx, "private").into(),
            )];
            let op = func::func(
                self.ctx,
                StringAttribute::new(self.ctx, &name),
                TypeAttribute::new(func_type.into()),
                Region::new(),
                private,
                self.loc,
            );
            self.module.body().append_operation(op);
            // Register as a known function signature so call sites can find it.
            self.funcs.entry(name).or_insert_with(|| FuncSig {
                param_types: vec![i64_type; n_params],
                return_type: Some(i64_type),
                has_rest: false,
                has_this_param: false,
            });
            return Ok(());
        }
        // Skip TypeScript overload signatures (declarations without a body).
        if func.body.is_none() { return Ok(()); }
        let raw_name = name_override.map(|s| s.to_string()).unwrap_or_else(|| id.name.to_string());
        // "main" conflicts with the LLVM entry point emitted by lower_main_function.
        let name = if raw_name == "main" { "__user_main".to_string() } else { raw_name };
        // Skip re-emission when multiple imported modules declare the same function name.
        if !self.emitted_functions.insert(name.clone()) {
            return Ok(());
        }
        let i32_type = self.i32_type();
        let i64_type = self.i64_type();
        // All functions return i64 (NaN-boxed TsVal) so they can return any value including heap objects.
        let return_type = i64_type;

        let explicit_rest = func.params.rest.is_some();
        let implicit_rest = !explicit_rest && func.body.as_ref().map_or(false, |b| {
            crate::lowering::expressions::body_uses_arguments(&b.statements)
        });
        let has_rest = explicit_rest || implicit_rest;
        let has_this_param = func.this_param.is_some();
        // Use i64 for all params to support NaN-boxed values (including `undefined` for defaults).
        // If has_this_param, MLIR param 0 is `this`; regular params start at index `this_offset`.
        let this_offset: usize = if has_this_param { 1 } else { 0 };
        let n_params = func.params.items.len()
            + if has_rest { 1 } else { 0 }
            + this_offset;
        let param_specs: Vec<(melior::ir::Type<'c>, Location<'c>)> =
            (0..n_params).map(|_| (i64_type, self.loc)).collect();
        let func_type = FunctionType::new(
            self.ctx, &vec![i64_type; n_params], &[return_type],
        );

        let region = Region::new();
        let entry  = region.append_block(Block::new(&param_specs));

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        let mut param_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        // If the function has a `this` parameter, bind MLIR param 0 to "this" in scope.
        if has_this_param {
            let this_val: Value<'_, '_> = entry.argument(0)?.into();
            scope.insert("this".to_string(), this_val);
            param_names.insert("this".to_string());
        }
        // current_block tracks the live basic block as parameter defaults may create new blocks.
        let mut current_block = entry;
        for (i, param) in func.params.items.iter().enumerate() {
            let param_val: Value<'_, '_> = entry.argument(i + this_offset)?.into();
            match &param.pattern {
                BindingPattern::BindingIdentifier(id) => {
                    let pname = id.name.to_string();
                    scope.insert(pname.clone(), param_val);
                    param_names.insert(pname);
                }
                BindingPattern::ObjectPattern(_) | BindingPattern::ArrayPattern(_) => {
                    // function foo({ x, y: { z } }: ...) or foo([a, [b, c]]: ...) — recursive destructure
                    let param_i64 = self.ensure_i64(param_val, current_block)?;
                    let scope_before: std::collections::HashSet<String> = scope.keys().cloned().collect();
                    current_block = self.lower_bind_pattern(&param.pattern, param_i64, current_block, &region, &mut scope)?;
                    // Track newly bound names as param names.
                    for name in scope.keys() {
                        if !scope_before.contains(name) {
                            param_names.insert(name.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        // Bind the rest parameter (last MLIR param) as a TsArray in scope.
        if let Some(rest_param) = &func.params.rest {
            if let BindingPattern::BindingIdentifier(rest_id) = &rest_param.rest.argument {
                let rest_arg_idx = func.params.items.len() + this_offset;
                let rest_val: Value<'_, '_> = entry.argument(rest_arg_idx)?.into();
                let rname = rest_id.name.to_string();
                scope.insert(rname.clone(), rest_val);
                param_names.insert(rname);
            }
        } else if implicit_rest {
            // Implicit rest: body uses `arguments` but no explicit rest param declared.
            // The last MLIR param is the bundled-args TsArray; bind it as `arguments`.
            let rest_arg_idx = func.params.items.len() + this_offset;
            let rest_val: Value<'_, '_> = entry.argument(rest_arg_idx)?.into();
            scope.insert("arguments".to_string(), rest_val);
            param_names.insert("arguments".to_string());
        }
        let saved_fn_params = std::mem::replace(&mut self.current_fn_params, param_names);


        // Inject module-level global variables into scope via ts_get_module_global.
        // This allows module-level functions to access module-level non-function consts
        // and enables inner closures to capture them correctly.
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

        // Emit default parameter checks: if param === undefined, use initializer.
        for (i, param) in func.params.items.iter().enumerate() {
            let Some(init_expr) = &param.initializer else { continue };
            let BindingPattern::BindingIdentifier(id) = &param.pattern else { continue };
            let param_name = id.name.to_string();

            let param_val: Value<'_, '_> = entry.argument(i + this_offset)?.into();
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
        // Implicit return value is always UNDEFINED for regular functions.
        // Explicit `return expr` is handled by lower_return_statement which emits func.return directly.
        let result_value: Value<'_, '_> = entry
            .append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(return_type, 0x7FF8_0000_0000_0000u64 as i64).into(),
                self.loc,
            ))
            .result(0)?.into();

        let saved_fn_return_type = self.fn_return_type;
        let saved_is_async = self.is_async;
        let saved_is_generator = self.is_generator;
        self.fn_return_type = return_type;
        self.is_async = func.r#async;
        self.is_generator = func.generator;

        // For generator functions, allocate the yields array at function entry.
        if func.generator {
            let zero_i32: Value<'_, '_> = current_block.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i32_type, 0).into(),
                self.loc,
            )).result(0)?.into();
            let yields_arr: Value<'_, '_> = current_block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_arr_new"),
                &[zero_i32], &[i64_type], self.loc,
            )).result(0)?.into();
            scope.insert("__generator_yields".to_string(), yields_arr);
        }

        if let Some(body) = &func.body {
            // Compute cell_vars: local variables mutated in nested closures (need heap cell boxing).
            let cell_vars_impl = crate::lowering::expressions::compute_cell_vars_for_body(&body.statements);
            let saved_cell_vars = std::mem::replace(&mut self.cell_vars, cell_vars_impl);
            let saved_cell_captures = std::mem::replace(&mut self.cell_captures, std::collections::HashSet::new());
            let mut sv_impl = crate::lowering::expressions::compute_scalar_vars_for_body(&body.statements);
            sv_impl.retain(|v| !self.cell_vars.contains(v));
            // Parameters with scalar TypeScript type annotations are also scalar.
            for param in &func.params.items {
                if let BindingPattern::BindingIdentifier(id) = &param.pattern {
                    if let Some(ann) = &param.type_annotation {
                        if crate::lowering::ts_type_is_scalar(&ann.type_annotation) {
                            sv_impl.insert(id.name.to_string());
                        }
                    }
                }
            }
            let saved_scalar_vars_impl = std::mem::replace(&mut self.scalar_vars, sv_impl);
            let mut nea_impl = crate::lowering::expressions::compute_non_escaping_allocs(&body.statements);
            nea_impl.retain(|v| !self.cell_vars.contains(v));
            let saved_non_escaping_impl = std::mem::replace(&mut self.non_escaping_allocs, nea_impl);

            // Pre-seed ALL local bindings (vars + inner function names) as undefined.
            let undef_placeholder: Value<'_, '_> = current_block.append_operation(arith::constant(
                self.ctx,
                IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(),
                self.loc,
            )).result(0)?.into();
            for stmt in &body.statements {
                if let Statement::FunctionDeclaration(inner_fn) = stmt {
                    if let Some(fn_id) = &inner_fn.id {
                        let fn_name = fn_id.name.to_string();
                        if !scope.contains_key(&fn_name) {
                            scope.insert(fn_name, undef_placeholder);
                        }
                    }
                }
            }

            // Process all statements in source order. FunctionDeclarations are handled inline.
            for stmt in &body.statements {
                if let Statement::FunctionDeclaration(inner_fn) = stmt {
                    let Some(fn_id) = &inner_fn.id else { continue };
                    let fn_name = fn_id.name.to_string();
                    let inner_params: Vec<&oxc_ast::ast::FormalParameter<'_>> =
                        inner_fn.params.items.iter().collect();
                    let inner_body = inner_fn.body.as_deref();
                    let inner_rest = inner_fn.params.rest.as_ref()
                        .and_then(|r| if let BindingPattern::BindingIdentifier(id) = &r.rest.argument {
                            Some(id.name.as_str())
                        } else { None });
                    let saved_async = self.is_async;
                    self.is_async = inner_fn.r#async;
                    let (fn_val, nb) = self.lower_arrow_like(
                        &inner_params,
                        inner_rest,
                        inner_body,
                        None,
                        current_block,
                        &region,
                        &mut scope,
                    )?;
                    self.is_async = saved_async;
                    current_block = nb;

                    // Fix up self-reference: if fn_name appears in its own free vars (recursion),
                    // the env slot was set to undefined. Patch it with the actual closure value.
                    {
                        use crate::lowering::expressions::collect_free_vars_stmts;
                        let mut inner_outer_keys: std::collections::HashSet<String> =
                            scope.keys().cloned().collect();
                        let mut inner_param_set: std::collections::HashSet<String> =
                            std::collections::HashSet::new();
                        for p in &inner_fn.params.items {
                            if let BindingPattern::BindingIdentifier(id) = &p.pattern {
                                inner_param_set.insert(id.name.to_string());
                            }
                        }
                        inner_outer_keys.insert(fn_name.clone());
                        let mut inner_free_vars: Vec<String> = Vec::new();
                        if let Some(inner_body_ref) = inner_fn.body.as_deref() {
                            collect_free_vars_stmts(
                                &inner_body_ref.statements,
                                &inner_param_set,
                                &inner_outer_keys,
                                &mut inner_free_vars,
                            );
                            let mut seen = std::collections::HashSet::new();
                            inner_free_vars.retain(|v| seen.insert(v.clone()));
                        }
                        if let Some(self_idx) = inner_free_vars.iter().position(|v| v == &fn_name) {
                            let env_arr = current_block.append_operation(func::call(
                                self.ctx,
                                FlatSymbolRefAttribute::new(self.ctx, "ts_closure_get_env"),
                                &[fn_val], &[i64_type], self.loc,
                            )).result(0)?.into();
                            let self_idx_val = current_block.append_operation(arith::constant(
                                self.ctx,
                                IntegerAttribute::new(i32_type, self_idx as i64).into(),
                                self.loc,
                            )).result(0)?.into();
                            current_block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_arr_set"),
                                &[env_arr, self_idx_val, fn_val], &[], self.loc,
                            ));
                            current_block.append_operation(func::call(
                                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                                &[env_arr], &[], self.loc,
                            ));
                        }
                    }

                    scope.insert(fn_name, fn_val);
                } else {
                    let (_, next) = self.lower_statement(stmt, current_block, &region, &mut scope, &[])?;
                    current_block = next;
                }
            }
            self.cell_vars = saved_cell_vars;
            self.cell_captures = saved_cell_captures;
            self.scalar_vars = saved_scalar_vars_impl;
            self.non_escaping_allocs = saved_non_escaping_impl;
        }
        // ARC: release scope variables before final return (skip parameters,
        // scalar vars, arena-allocated vars, and generator yields array).
        for (name, v) in &scope {
            if self.current_fn_params.contains(name) { continue; }
            if func.generator && name == "__generator_yields" { continue; }
            if self.scalar_vars.contains(name.as_str()) { continue; }
            if self.non_escaping_allocs.contains(name.as_str()) { continue; }
            let v_i64 = self.ensure_i64(*v, current_block)?;
            current_block.append_operation(func::call(
                self.ctx,
                FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[v_i64], &[], self.loc,
            ));
        }

        self.current_fn_params = saved_fn_params;

        // Async: wrap the implicit return value in a resolved Promise.
        // ARC: ts_promise_resolve retains val internally; release our owned ref after the call.
        if current_block.terminator().is_none() && func.generator {
            // Generator: return the collected yields array.
            let yields_val = scope.get("__generator_yields")
                .copied()
                .unwrap_or(result_value);
            let yields_i64 = self.ensure_i64(yields_val, current_block)?;
            current_block.append_operation(func::r#return(&[yields_i64], self.loc));
        } else if current_block.terminator().is_none() && func.r#async {
            let val_i64 = self.ensure_i64(result_value, current_block)?;
            let promise: Value<'_, '_> = current_block
                .append_operation(func::call(
                    self.ctx,
                    FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                    &[val_i64], &[i64_type], self.loc,
                ))
                .result(0)?.into();
            current_block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[val_i64], &[], self.loc,
            ));
            current_block.append_operation(func::r#return(&[promise], self.loc));
        } else {
            self.terminate_with_return(current_block, result_value)?;
        }

        self.is_async = saved_is_async;
        self.is_generator = saved_is_generator;
        self.fn_return_type = saved_fn_return_type;

        let private_vis = &[(
            Identifier::new(self.ctx, "sym_visibility"),
            StringAttribute::new(self.ctx, "private").into(),
        )];
        let op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, &name),
            TypeAttribute::new(func_type.into()),
            region,
            private_vis,
            self.loc,
        );
        self.module.body().append_operation(op);

        self.funcs.insert(name, FuncSig {
            param_types: vec![i64_type; param_specs.len()],
            return_type: Some(return_type),
            has_rest,
            has_this_param,
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
        is_expr_body: bool,
    ) -> Result<()> {
        // Skip re-emission when multiple imported modules declare the same function name.
        if !self.emitted_functions.insert(name.to_string()) {
            return Ok(());
        }
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

        // Track names of borrowed param refs (simple identifiers and rest) so we can
        // skip releasing them in the end-of-function scope cleanup — callers own them.
        let mut borrowed_param_names: std::collections::HashSet<String> = std::collections::HashSet::new();

        let mut scope: HashMap<String, Value<'_, '_>> = HashMap::new();
        let mut current_block = entry;
        for (i, param) in params.iter().enumerate() {
            let arg_val: Value<'_, '_> = entry.argument(i)?.into();
            match &param.pattern {
                BindingPattern::BindingIdentifier(id) => {
                    let name = id.name.to_string();
                    borrowed_param_names.insert(name.clone());
                    scope.insert(name, arg_val);
                }
                BindingPattern::ArrayPattern(_) | BindingPattern::ObjectPattern(_) => {
                    // Recursive destructuring in function/closure params.
                    let arg_i64 = self.ensure_i64(arg_val, current_block)?;
                    current_block = self.lower_bind_pattern(&param.pattern, arg_i64, current_block, &region, &mut scope)?;
                }
                _ => {}
            }
        }
        if let Some(rest_name) = rest_param_name {
            let rest_idx = params.len();
            let rest_str = rest_name.to_string();
            borrowed_param_names.insert(rest_str.clone());
            scope.insert(rest_str, entry.argument(rest_idx)?.into());
        }

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
        let saved_fn_return_type = self.fn_return_type;
        let saved_is_async = self.is_async;
        let saved_fn_params = std::mem::replace(&mut self.current_fn_params, borrowed_param_names.clone());
        self.fn_return_type = return_type;
        self.is_async = is_async;
        if let Some(body) = body {
            for stmt in &body.statements {
                // For expression-body arrows (=> expr), the single ExpressionStatement
                // IS the return value. Do NOT release it; preserve the owned reference.
                if is_expr_body {
                    if let oxc_ast::ast::Statement::ExpressionStatement(es) = stmt {
                        let (val_opt, nb) = self.lower_expression(&es.expression, current_block, &region, &mut scope)?;
                        current_block = nb;
                        if let Some(v) = val_opt {
                            result_value = self.ensure_i64(v, current_block)?;
                        }
                        continue;
                    }
                }
                let (val, next) = self.lower_statement(stmt, current_block, &region, &mut scope, &[])?;
                current_block = next;
                if let Some(v) = val { result_value = v; }
            }
        }
        self.is_async = saved_is_async;
        self.current_fn_params = saved_fn_params;

        for (name, v) in &scope {
            // Skip borrowed parameter refs — the caller owns them.
            if borrowed_param_names.contains(name.as_str()) { continue; }
            // Skip scalar vars — retain/release are no-ops for non-pointer values.
            if self.scalar_vars.contains(name.as_str()) { continue; }
            // Skip arena-allocated vars — freed in bulk by arena_exit at fiber end.
            if self.non_escaping_allocs.contains(name.as_str()) { continue; }
            let v_i64 = self.ensure_i64(*v, current_block)?;
            current_block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_release_val"),
                &[v_i64], &[], self.loc,
            ));
        }

        if current_block.terminator().is_none() && is_async {
            // Async implicit return: always resolve with UNDEFINED (void return).
            // Do NOT use result_value — ExpressionStatement already released it.
            let undef_i64: Value<'_, '_> = current_block.append_operation(arith::constant(
                self.ctx, IntegerAttribute::new(i64_type, 0x7FF8_0000_0000_0000u64 as i64).into(), self.loc,
            )).result(0)?.into();
            let promise: Value<'_, '_> = current_block.append_operation(func::call(
                self.ctx, FlatSymbolRefAttribute::new(self.ctx, "ts_promise_resolve"),
                &[undef_i64], &[i64_type], self.loc,
            )).result(0)?.into();
            current_block.append_operation(func::r#return(&[promise], self.loc));
        } else {
            self.terminate_with_return(current_block, result_value)?;
        }
        self.fn_return_type = saved_fn_return_type;

        let op = func::func(
            self.ctx,
            StringAttribute::new(self.ctx, name),
            TypeAttribute::new(func_type.into()),
            region,
            &[(
                Identifier::new(self.ctx, "sym_visibility"),
                StringAttribute::new(self.ctx, "private").into(),
            )],
            self.loc,
        );
        self.module.body().append_operation(op);
        Ok(())
    }
}

// ── ARC elision helpers ───────────────────────────────────────────────────────

/// Returns true if a TypeScript type annotation is provably always scalar
/// (number, boolean, null, undefined, void, never, numeric/boolean literal types).
/// Used to classify function parameters as scalar without needing body analysis.
pub(super) fn ts_type_is_scalar(ty: &oxc_ast::ast::TSType<'_>) -> bool {
    use oxc_ast::ast::{TSType, TSLiteral};
    match ty {
        TSType::TSNumberKeyword(_)
        | TSType::TSBooleanKeyword(_)
        | TSType::TSNullKeyword(_)
        | TSType::TSUndefinedKeyword(_)
        | TSType::TSVoidKeyword(_)
        | TSType::TSNeverKeyword(_)
        | TSType::TSBigIntKeyword(_) => true,
        // Numeric/boolean literal types (e.g. `0 | 1`, `true | false`)
        TSType::TSLiteralType(lit) => matches!(
            lit.literal,
            TSLiteral::NumericLiteral(_) | TSLiteral::BooleanLiteral(_)
        ),
        _ => false,
    }
}

/// Returns true if the expression's result is provably a scalar NaN-box value
/// (TAG_INT, TAG_BOOL, TAG_NULL, TAG_UNDEFINED) regardless of runtime inputs —
/// i.e., the result can never be a heap pointer.  Used to elide `ts_retain_val`
/// / `ts_release_val` call overhead (~3–5 ns per call on ARM64).
///
/// Conservative: returns `false` when unsure.  Does NOT check `scalar_vars`;
/// use `Lowerer::expr_result_is_scalar` for that.
pub(super) fn expr_is_definitely_scalar(expr: &Expression) -> bool {
    use oxc_ast::ast::{BinaryOperator as B, UnaryOperator as U};
    match expr {
        Expression::NumericLiteral(_) | Expression::BooleanLiteral(_) | Expression::NullLiteral(_) => true,
        Expression::Identifier(id) => matches!(id.name.as_str(), "undefined" | "NaN" | "Infinity"),
        Expression::UnaryExpression(un) => matches!(
            un.operator,
            U::UnaryNegation | U::UnaryPlus | U::BitwiseNot | U::LogicalNot
        ),
        Expression::BinaryExpression(bin) => match bin.operator {
            // Comparisons always return boolean (scalar).
            B::LessThan | B::GreaterThan | B::LessEqualThan | B::GreaterEqualThan
            | B::Equality | B::Inequality | B::StrictEquality | B::StrictInequality
            | B::Instanceof | B::In => true,
            // Numeric arithmetic (not Addition which may string-concat) always returns number.
            B::Subtraction | B::Multiplication | B::Division | B::Remainder | B::Exponential => true,
            // Bitwise ops always return integer (scalar).
            B::BitwiseOR | B::BitwiseAnd | B::BitwiseXOR
            | B::ShiftLeft | B::ShiftRight | B::ShiftRightZeroFill => true,
            // Addition is scalar only when both operands are scalar (no string concat possible).
            B::Addition => expr_is_definitely_scalar(&bin.left) && expr_is_definitely_scalar(&bin.right),
            _ => false,
        },
        // TS wrappers are transparent.
        Expression::TSAsExpression(e)           => expr_is_definitely_scalar(&e.expression),
        Expression::TSNonNullExpression(e)      => expr_is_definitely_scalar(&e.expression),
        Expression::TSSatisfiesExpression(e)    => expr_is_definitely_scalar(&e.expression),
        Expression::TSTypeAssertion(e)          => expr_is_definitely_scalar(&e.expression),
        _ => false,
    }
}
