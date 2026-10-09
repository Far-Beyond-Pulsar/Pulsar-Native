//! Checking a TypeScript class and lowering it to a script module.
//!
//! One pass does both: every expression is typed as it is lowered, against
//! the script type system and the native registry, so what compiles is
//! exactly what the VM verifier accepts. See the crate docs for the subset.

use std::collections::HashMap;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, AssignmentOperator, AssignmentTarget, BinaryOperator, BindingPattern, Class,
    ClassElement, ClassHeritage, Declaration, ExportDefaultDeclarationKind, Expression,
    ForStatementInit, FormalParameter, Function, LogicalOperator, MethodDefinitionKind,
    PropertyKey, SimpleAssignmentTarget, Statement, TSAccessibility, TSType, TSTypeName,
    UnaryOperator, UpdateOperator, VariableDeclarationKind,
};
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};
use pulsar_script_vm::{
    verify, BinOp, Constant, DebugInfo, Function as VmFunction, Import, Instr, Module,
    NativeRegistry, Param, Reg, SourceLoc, Type, TypeRegistry, UnOp, Variable,
};

use crate::declarations::callable;
use crate::diagnostic::{Diagnostic, LineIndex};
use crate::schema::{ClassSchema, DeclaredField};

/// A class to compile.
pub struct ClassSource<'a> {
    /// The class's name: its directory name. The default-exported class
    /// must have the same name.
    pub class_name: &'a str,
    /// The source file's name, for diagnostics and debug info.
    pub file: &'a str,
    pub source: &'a str,
    /// The schema the class had when it last compiled, if it did.
    pub schema: Option<&'a ClassSchema>,
}

/// The result of compiling a class.
#[derive(Debug)]
pub struct Compiled {
    /// `None` if there is any error.
    pub module: Option<Module>,
    pub diagnostics: Vec<Diagnostic>,
    /// The schema to store next to the source. `Some` whenever the fields
    /// could be reconciled, even if the class has other errors.
    pub schema: Option<ClassSchema>,
}

pub fn compile_class(src: &ClassSource<'_>, natives: &NativeRegistry) -> Compiled {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, src.source, SourceType::ts()).parse();
    let lines = LineIndex::new(src.source);
    let mut cx = Cx::new(src, natives, lines);

    for error in parsed.diagnostics.errors() {
        let offset = error.labels.first().map_or(0, |l| l.offset());
        let (line, column) = cx.lines.position(src.source, offset);
        cx.diagnostics
            .push(Diagnostic::error(error.message.to_string(), line, column));
    }
    if !cx.diagnostics.is_empty() {
        return Compiled {
            module: None,
            diagnostics: cx.diagnostics,
            schema: None,
        };
    }

    let Some(class) = cx.find_class(&parsed.program.body) else {
        return Compiled {
            module: None,
            diagnostics: cx.diagnostics,
            schema: None,
        };
    };
    cx.compile(class)
}

// ---- state ------------------------------------------------------------------

#[derive(Clone)]
struct MethodSig {
    index: u32,
    params: Vec<Type>,
    ret: Type,
    is_async: bool,
}

#[derive(Clone)]
struct Local {
    reg: Reg,
    ty: Type,
    mutable: bool,
}

#[derive(Default)]
struct LoopCtx {
    breaks: Vec<usize>,
    continues: Vec<usize>,
}

/// The function being lowered.
struct Fx {
    regs: Vec<Type>,
    code: Vec<Instr>,
    debug: DebugInfo,
    scopes: Vec<HashMap<String, Local>>,
    loops: Vec<LoopCtx>,
    ret: Type,
    is_async: bool,
    loc: Option<SourceLoc>,
}

struct Cx<'a> {
    src: &'a ClassSource<'a>,
    natives: &'a NativeRegistry,
    lines: LineIndex,
    diagnostics: Vec<Diagnostic>,
    fields: HashMap<String, (u32, Type)>,
    methods: HashMap<String, MethodSig>,
    constants: Vec<Constant>,
    imports: Vec<Import>,
    import_index: HashMap<String, u32>,
}

type Val = (Reg, Type);

impl<'a> Cx<'a> {
    fn new(src: &'a ClassSource<'a>, natives: &'a NativeRegistry, lines: LineIndex) -> Self {
        Self {
            src,
            natives,
            lines,
            diagnostics: Vec::new(),
            fields: HashMap::new(),
            methods: HashMap::new(),
            constants: Vec::new(),
            imports: Vec::new(),
            import_index: HashMap::new(),
        }
    }

    fn err(&mut self, span: Span, message: impl Into<String>) {
        let (line, column) = self.lines.position(self.src.source, span.start);
        self.diagnostics
            .push(Diagnostic::error(message, line, column));
    }

    fn unsupported(&mut self, span: Span, what: &str) {
        self.err(span, format!("{what} is not supported in script classes"));
    }

    // ---- finding the class ---------------------------------------------------

    fn find_class<'p>(&mut self, body: &'p [Statement<'p>]) -> Option<&'p Class<'p>> {
        let mut found: Option<&'p Class<'p>> = None;
        for statement in body {
            let class = match statement {
                Statement::ExportDefaultDeclaration(e) => match &e.declaration {
                    ExportDefaultDeclarationKind::ClassDeclaration(c) => Some(&**c),
                    _ => {
                        self.err(e.span, "only a class may be the default export");
                        None
                    }
                },
                Statement::ExportDeclaration(e) => match &e.declaration {
                    Declaration::ClassDeclaration(c) => Some(&**c),
                    Declaration::TSTypeAliasDeclaration(_)
                    | Declaration::TSInterfaceDeclaration(_) => None,
                    _ => {
                        self.err(e.span, "only the script class may be exported");
                        None
                    }
                },
                Statement::ExportNamedDeclaration(e) => {
                    self.err(e.span, "only the script class may be exported");
                    None
                }
                Statement::ClassDeclaration(c) => Some(&**c),
                // Types are checked by tooling against the generated declarations; they carry no behaviour.
                Statement::TSTypeAliasDeclaration(_)
                | Statement::TSInterfaceDeclaration(_)
                | Statement::EmptyStatement(_) => None,
                Statement::ImportDeclaration(i) => {
                    self.err(i.span, "imports are not supported: everything a class may use is declared by the generated declarations");
                    None
                }
                other => {
                    self.err(
                        other.span(),
                        "only the script class may appear at the top level",
                    );
                    None
                }
            };
            if let Some(class) = class {
                if found.is_some() {
                    self.err(class.span, "a script file declares exactly one class");
                } else {
                    found = Some(class);
                }
            }
        }
        if found.is_none() && self.diagnostics.is_empty() {
            self.diagnostics.push(Diagnostic::error(
                format!(
                    "no class found: declare `export default class {} {{ .. }}`",
                    self.src.class_name
                ),
                0,
                0,
            ));
        }
        found
    }

    // ---- types ---------------------------------------------------------------

    fn resolve_type(&mut self, ty: &TSType<'_>) -> Option<Type> {
        let span = ty.span();
        match ty {
            TSType::TSNumberKeyword(_) => Some(Type::Float),
            TSType::TSBooleanKeyword(_) => Some(Type::Bool),
            TSType::TSStringKeyword(_) => Some(Type::Str),
            TSType::TSVoidKeyword(_) => Some(Type::Unit),
            TSType::TSParenthesizedType(p) => self.resolve_type(&p.type_annotation),
            TSType::TSArrayType(array) => Some(Type::list(self.resolve_type(&array.element_type)?)),
            TSType::TSTupleType(tuple) => {
                let mut items = Vec::new();
                for element in &tuple.element_types {
                    items.push(self.resolve_type(element.to_ts_type())?);
                }
                Some(Type::Tuple(items))
            }
            TSType::TSTypeReference(reference) => {
                let TSTypeName::IdentifierReference(name) = &reference.type_name else {
                    self.err(span, "qualified type names are not supported");
                    return None;
                };
                let name = name.name.as_str();
                let types = TypeRegistry::global();
                match name {
                    "int" => Some(Type::Int),
                    "float" => Some(Type::Float),
                    "Entity" => Some(Type::Entity),
                    "Map" => {
                        let args: Vec<_> = reference
                            .type_arguments
                            .iter()
                            .flat_map(|a| a.params.iter())
                            .collect();
                        let [key, value] = args[..] else {
                            self.err(
                                span,
                                "`Map` takes a key and a value type: `Map<string, int>`",
                            );
                            return None;
                        };
                        let key = self.resolve_type(key)?;
                        if !key.is_key() {
                            self.err(span, "a map key must be boolean, int or string");
                            return None;
                        }
                        Some(Type::map(key, self.resolve_type(value)?))
                    }
                    _ if types.component(name).is_some() => Some(Type::Component(name.to_owned())),
                    _ if types.value_types().any(|v| v == name) => {
                        Some(Type::Object(name.to_owned()))
                    }
                    _ => {
                        self.err(span, format!("unknown type `{name}`"));
                        None
                    }
                }
            }
            _ => {
                self.err(span, "this type is not supported: use number, int, boolean, string, Entity, or a type the engine provides");
                None
            }
        }
    }

    fn annotation(
        &mut self,
        annotation: &Option<oxc_allocator::Box<'_, oxc_ast::ast::TSTypeAnnotation<'_>>>,
    ) -> Option<Type> {
        annotation
            .as_ref()
            .and_then(|a| self.resolve_type(&a.type_annotation))
    }

    // ---- constants and imports -----------------------------------------------

    fn konst(&mut self, constant: Constant) -> u32 {
        if let Some(i) = self.constants.iter().position(|c| *c == constant) {
            return i as u32;
        }
        self.constants.push(constant);
        (self.constants.len() - 1) as u32
    }

    fn import(&mut self, name: &str) -> Option<u32> {
        if let Some(&i) = self.import_index.get(name) {
            return Some(i);
        }
        let native = self.natives.get(name)?;
        self.imports.push(Import {
            name: name.to_owned(),
            sig: native.sig.clone(),
        });
        let index = (self.imports.len() - 1) as u32;
        self.import_index.insert(name.to_owned(), index);
        Some(index)
    }

    /// The native for `namespace.member`, accepting the sanitized spelling
    /// (`string_`, `new_`) the generated declarations use.
    fn native(
        &self,
        namespace: &str,
        member: &str,
    ) -> Option<std::sync::Arc<pulsar_script_vm::NativeFn>> {
        let candidates = |s: &str| {
            let mut v = vec![s.to_owned()];
            if let Some(stripped) = s.strip_suffix('_') {
                v.push(stripped.to_owned());
            }
            v
        };
        for ns in candidates(namespace) {
            for name in candidates(member) {
                if let Some(native) = self.natives.get(&format!("{ns}::{name}")) {
                    if callable(native) {
                        return Some(std::sync::Arc::clone(native));
                    }
                }
            }
        }
        None
    }

    // ---- compiling the class ---------------------------------------------------

    fn compile(mut self, class: &Class<'_>) -> Compiled {
        let class_name = self.src.class_name.to_owned();
        match &class.id {
            Some(id) if id.name.as_str() == class_name => {}
            Some(id) => self.err(
                id.span,
                format!(
                    "the class is named `{}` but its directory is `{class_name}`: they must match",
                    id.name
                ),
            ),
            None => self.err(
                class.span,
                format!("the class needs a name: `{class_name}`"),
            ),
        }
        if !class.decorators.is_empty() {
            self.unsupported(class.span, "class decorators");
        }
        if let Some(ClassHeritage { .. }) = &class.heritage {
            // `extends ScriptClass` is accepted (it gives editors `entity` and `time`).
            let ok = matches!(&class.heritage, Some(h) if matches!(&h.expression, Expression::Identifier(i) if i.name.as_str() == "ScriptClass"));
            if !ok {
                self.err(class.span, "a script class may only extend `ScriptClass`");
            }
        }
        if class.type_parameters.is_some() || !class.implements.is_empty() {
            self.unsupported(class.span, "generics and `implements`");
        }

        // Fields.
        let mut declared = Vec::new();
        let mut variables: Vec<(String, Type, Option<Constant>)> = Vec::new();
        for element in &class.body.body {
            if let ClassElement::PropertyDefinition(property) = element {
                if let Some((name, ty, default, renamed_from)) = self.field(property) {
                    if variables.iter().any(|(n, ..)| *n == name) {
                        self.err(property.span, format!("field `{name}` is declared twice"));
                        continue;
                    }
                    declared.push(DeclaredField {
                        name: name.clone(),
                        ty: ty.to_string(),
                        renamed_from,
                    });
                    self.fields
                        .insert(name.clone(), (variables.len() as u32, ty.clone()));
                    variables.push((name, ty, default));
                }
            }
        }
        let schema = match ClassSchema::reconcile(self.src.schema, &declared) {
            Ok(schema) => Some(schema),
            Err(problems) => {
                for message in problems {
                    self.diagnostics.push(Diagnostic::error(message, 0, 0));
                }
                None
            }
        };

        // Method signatures first, so calls can refer to methods declared later.
        let mut methods: Vec<(&Function<'_>, String, bool)> = Vec::new();
        for element in &class.body.body {
            match element {
                ClassElement::MethodDefinition(method) => {
                    if method.kind != MethodDefinitionKind::Method {
                        let what = match method.kind {
                            MethodDefinitionKind::Constructor => {
                                "a constructor: initialise fields where they are declared"
                            }
                            _ => "accessors",
                        };
                        self.unsupported(method.span, what);
                        continue;
                    }
                    if method.r#static || method.computed || !method.decorators.is_empty() {
                        self.unsupported(method.span, "static, computed or decorated methods");
                        continue;
                    }
                    let PropertyKey::StaticIdentifier(key) = &method.key else {
                        self.unsupported(method.span, "this method name");
                        continue;
                    };
                    let name = key.name.as_str().to_owned();
                    let exported = !name.starts_with('_')
                        && !matches!(
                            method.accessibility,
                            Some(TSAccessibility::Private | TSAccessibility::Protected)
                        );
                    if self.methods.contains_key(&name) {
                        self.err(method.span, format!("method `{name}` is declared twice"));
                        continue;
                    }
                    if let Some(sig) = self.method_sig(&method.value, methods.len() as u32) {
                        self.methods.insert(name.clone(), sig);
                        methods.push((&method.value, name, exported));
                    }
                }
                ClassElement::PropertyDefinition(_) => {}
                other => self.unsupported(
                    other.span(),
                    "static blocks, accessors and index signatures",
                ),
            }
        }
        self.check_lifecycle();

        // Lower each method.
        let mut functions = Vec::new();
        for (function, name, exported) in &methods {
            let sig = self.methods[name].clone();
            if let Some(lowered) = self.lower_method(function, name, *exported, &sig) {
                functions.push(lowered);
            }
        }

        if self.diagnostics.iter().any(Diagnostic::is_error) {
            return Compiled {
                module: None,
                diagnostics: self.diagnostics,
                schema,
            };
        }
        let schema = schema.expect("reconciled when there are no errors");
        let mut module = Module::new(class_name);
        module.class_version = schema.version;
        module.constants = std::mem::take(&mut self.constants);
        module.imports = std::mem::take(&mut self.imports);
        module.variables = variables
            .into_iter()
            .map(|(name, ty, default)| Variable {
                id: schema.id_of(&name).map(str::to_owned),
                name,
                ty,
                default,
            })
            .collect();
        module.functions = functions;
        if let Err(error) = verify(&module) {
            self.diagnostics.push(Diagnostic::error(
                format!("internal error: the compiled module does not verify: {error}"),
                0,
                0,
            ));
            return Compiled {
                module: None,
                diagnostics: self.diagnostics,
                schema: Some(schema),
            };
        }
        Compiled {
            module: Some(module),
            diagnostics: self.diagnostics,
            schema: Some(schema),
        }
    }

    /// `begin_play()`, `tick(dt: number)`, `end_play()` and `migrate(from: int)` have fixed signatures.
    fn check_lifecycle(&mut self) {
        let expect = [
            ("begin_play", vec![], "begin_play(): void"),
            ("end_play", vec![], "end_play(): void"),
            ("tick", vec![Type::Float], "tick(delta: number): void"),
            (
                "migrate",
                vec![Type::Int],
                "migrate(fromVersion: int): void",
            ),
        ];
        for (name, params, text) in expect {
            if let Some(sig) = self.methods.get(name).cloned() {
                if sig.params != params
                    || sig.ret != Type::Unit
                    || sig.is_async && name == "migrate"
                {
                    self.diagnostics.push(Diagnostic::error(
                        format!("`{name}` must be declared `{text}`"),
                        0,
                        0,
                    ));
                }
            }
        }
    }

    fn field(
        &mut self,
        property: &oxc_ast::ast::PropertyDefinition<'_>,
    ) -> Option<(String, Type, Option<Constant>, Option<String>)> {
        if property.r#static || property.computed || property.declare {
            self.unsupported(property.span, "static, computed or `declare` fields");
            return None;
        }
        let PropertyKey::StaticIdentifier(key) = &property.key else {
            self.unsupported(property.span, "this field name");
            return None;
        };
        let name = key.name.as_str().to_owned();
        let mut renamed_from = None;
        for decorator in &property.decorators {
            match renamed_from_decorator(&decorator.expression) {
                Some(old) => renamed_from = Some(old),
                None => self.err(
                    decorator.span,
                    "the only decorator a field may have is `@renamedFrom(\"oldName\")`",
                ),
            }
        }
        let annotated = self.annotation(&property.type_annotation);
        if property.type_annotation.is_some() && annotated.is_none() {
            return None;
        }
        let (default, inferred) = match &property.value {
            Some(value) => {
                let (constant, ty) = self.const_value(value, annotated.as_ref())?;
                (Some(constant), Some(ty))
            }
            None => (None, None),
        };
        let ty = match (annotated, inferred) {
            (Some(a), Some(i)) if a != i => {
                self.err(
                    property.span,
                    format!(
                        "field `{name}` is `{}` but its initializer is `{}`",
                        crate::declarations::ts_type(&a),
                        crate::declarations::ts_type(&i)
                    ),
                );
                return None;
            }
            (Some(a), _) => a,
            (None, Some(i)) => i,
            (None, None) => {
                self.err(
                    property.span,
                    format!("field `{name}` needs a type annotation or an initializer"),
                );
                return None;
            }
        };
        Some((name, ty, default, renamed_from))
    }

    fn method_sig(&mut self, function: &Function<'_>, index: u32) -> Option<MethodSig> {
        if function.generator || function.type_parameters.is_some() || function.this_param.is_some()
        {
            self.unsupported(function.span, "generators, generics and `this` parameters");
            return None;
        }
        if function.params.rest.is_some() {
            self.unsupported(function.span, "rest parameters");
            return None;
        }
        let mut params = Vec::new();
        let mut ok = true;
        for parameter in &function.params.items {
            match self.param(parameter) {
                Some((_, ty)) => params.push(ty),
                None => ok = false,
            }
        }
        let ret = match &function.return_type {
            None => Type::Unit,
            Some(annotation) => match &annotation.type_annotation {
                TSType::TSTypeReference(r)
                    if function.r#async
                        && matches!(&r.type_name, TSTypeName::IdentifierReference(i) if i.name.as_str() == "Promise") =>
                {
                    let inner = r.type_arguments.as_ref().and_then(|a| a.params.first());
                    match inner {
                        Some(TSType::TSVoidKeyword(_)) | None => Type::Unit,
                        _ => {
                            self.err(annotation.span, "an async method returns `Promise<void>`");
                            ok = false;
                            Type::Unit
                        }
                    }
                }
                other => match self.resolve_type(other) {
                    Some(t) => t,
                    None => {
                        ok = false;
                        Type::Unit
                    }
                },
            },
        };
        if function.r#async && ret != Type::Unit {
            self.err(function.span, "an async method returns `Promise<void>`");
            ok = false;
        }
        ok.then_some(MethodSig {
            index,
            params,
            ret,
            is_async: function.r#async,
        })
    }

    fn param(&mut self, parameter: &FormalParameter<'_>) -> Option<(String, Type)> {
        let BindingPattern::BindingIdentifier(id) = &parameter.pattern else {
            self.unsupported(parameter.span, "destructured parameters");
            return None;
        };
        if parameter.initializer.is_some() || parameter.optional {
            self.unsupported(parameter.span, "default and optional parameters");
            return None;
        }
        let Some(annotation) = &parameter.type_annotation else {
            self.err(
                parameter.span,
                format!("parameter `{}` needs a type annotation", id.name),
            );
            return None;
        };
        let ty = self.resolve_type(&annotation.type_annotation)?;
        if ty == Type::Unit {
            self.err(parameter.span, "a parameter cannot be `void`");
            return None;
        }
        Some((id.name.as_str().to_owned(), ty))
    }

    // ---- constant expressions -----------------------------------------------------

    /// A field initializer: a literal, or `T.new_(numbers..)` of a value type.
    fn const_value(
        &mut self,
        expr: &Expression<'_>,
        expected: Option<&Type>,
    ) -> Option<(Constant, Type)> {
        match expr {
            Expression::ParenthesizedExpression(p) => self.const_value(&p.expression, expected),
            Expression::BooleanLiteral(b) => Some((Constant::Bool(b.value), Type::Bool)),
            Expression::StringLiteral(s) => {
                Some((Constant::Str(s.value.as_str().to_owned()), Type::Str))
            }
            Expression::NumericLiteral(n) => Some(number_constant(
                n.value,
                n.raw.as_ref().map(|r| r.as_str()),
                expected,
            )),
            Expression::UnaryExpression(u) if u.operator == UnaryOperator::UnaryNegation => {
                if let Expression::NumericLiteral(n) = &u.argument {
                    let (constant, ty) =
                        number_constant(-n.value, n.raw.as_ref().map(|r| r.as_str()), expected);
                    return Some((constant, ty));
                }
                self.err(expr.span(), "a field initializer must be a literal");
                None
            }
            Expression::CallExpression(call) => {
                let Expression::StaticMemberExpression(member) = &call.callee else {
                    self.err(expr.span(), "a field initializer must be a literal");
                    return None;
                };
                let Expression::Identifier(ty) = &member.object else {
                    self.err(expr.span(), "a field initializer must be a literal");
                    return None;
                };
                let name = ty.name.as_str();
                let is_value_type = TypeRegistry::global().value_types().any(|v| v == name);
                if !is_value_type || member.property.name.as_str().trim_end_matches('_') != "new" {
                    self.err(expr.span(), "a field initializer must be a literal or `Type.new_(numbers..)` of a value type");
                    return None;
                }
                let mut parts = Vec::new();
                for argument in &call.arguments {
                    match argument.as_expression().and_then(numeric_literal) {
                        Some(value) => parts.push(value),
                        None => {
                            self.err(
                                argument.span(),
                                "value-type initializers take number literals",
                            );
                            return None;
                        }
                    }
                }
                let json = format!(
                    "[{}]",
                    parts
                        .iter()
                        .map(f64::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                );
                if let Err(message) = TypeRegistry::global().decode_value(name, &json) {
                    self.err(expr.span(), format!("not a valid `{name}`: {message}"));
                    return None;
                }
                Some((
                    Constant::Value {
                        ty: name.to_owned(),
                        json,
                    },
                    Type::Object(name.to_owned()),
                ))
            }
            _ => {
                self.err(expr.span(), "a field initializer must be a literal");
                None
            }
        }
    }

    // ---- methods ---------------------------------------------------------------------

    fn lower_method(
        &mut self,
        function: &Function<'_>,
        name: &str,
        exported: bool,
        sig: &MethodSig,
    ) -> Option<VmFunction> {
        let Some(body) = &function.body else {
            self.err(function.span, format!("method `{name}` has no body"));
            return None;
        };
        let mut f = Fx {
            regs: Vec::new(),
            code: Vec::new(),
            debug: DebugInfo::default(),
            scopes: vec![HashMap::new()],
            loops: Vec::new(),
            ret: sig.ret.clone(),
            is_async: sig.is_async,
            loc: None,
        };
        for (parameter, ty) in function.params.items.iter().zip(&sig.params) {
            let BindingPattern::BindingIdentifier(id) = &parameter.pattern else {
                continue;
            };
            let reg = self.new_reg(&mut f, ty.clone());
            f.scopes[0].insert(
                id.name.as_str().to_owned(),
                Local {
                    reg,
                    ty: ty.clone(),
                    mutable: true,
                },
            );
        }
        for statement in &body.statements {
            self.statement(&mut f, statement);
        }
        // Every jump target must be an instruction, including the one past a
        // trailing `if` or loop, so a method always ends with one.
        let terminated = body.statements.iter().any(terminates);
        if sig.ret == Type::Unit {
            self.emit(&mut f, Instr::Return { value: None });
        } else if !terminated {
            self.err(
                function.span,
                format!("not all code paths of `{name}` return a value"),
            );
        } else {
            // Every path returns, so this is never reached.
            let here = Self::here(&f);
            self.emit(&mut f, Instr::Jump { target: here });
        }
        Some(VmFunction {
            name: name.to_owned(),
            exported,
            params: sig.params.clone(),
            ret: sig.ret.clone(),
            registers: f.regs,
            code: f.code,
            debug: (!f.debug.ranges.is_empty()).then_some(f.debug),
        })
    }

    fn new_reg(&mut self, f: &mut Fx, ty: Type) -> Reg {
        if f.regs.len() >= usize::from(Reg::MAX) {
            self.diagnostics.push(Diagnostic::error(
                "the method needs too many registers: split it up",
                0,
                0,
            ));
            return 0;
        }
        f.regs.push(ty);
        (f.regs.len() - 1) as Reg
    }

    fn at(&mut self, f: &mut Fx, span: Span) {
        let (line, column) = self.lines.position(self.src.source, span.start);
        f.loc = Some(SourceLoc {
            file: self.src.file.to_owned(),
            node: String::new(),
            line: Some(line),
            column: Some(column),
        });
    }

    fn emit(&mut self, f: &mut Fx, instr: Instr) -> usize {
        let pc = f.code.len();
        if let Some(loc) = &f.loc {
            f.debug.record(pc as u32, loc);
        }
        f.code.push(instr);
        pc
    }

    fn here(f: &Fx) -> u32 {
        f.code.len() as u32
    }

    fn patch(f: &mut Fx, at: usize, target: u32) {
        match &mut f.code[at] {
            Instr::Jump { target: t } => *t = target,
            other => unreachable!("patching {other:?}"),
        }
    }

    fn patch_branch(f: &mut Fx, at: usize, then: Option<u32>, otherwise: Option<u32>) {
        if let Instr::Branch {
            then: t,
            otherwise: o,
            ..
        } = &mut f.code[at]
        {
            if let Some(then) = then {
                *t = then;
            }
            if let Some(otherwise) = otherwise {
                *o = otherwise;
            }
        }
    }

    // ---- statements ----------------------------------------------------------------------

    fn statement(&mut self, f: &mut Fx, statement: &Statement<'_>) {
        self.at(f, statement.span());
        match statement {
            Statement::EmptyStatement(_) => {}
            Statement::BlockStatement(block) => {
                f.scopes.push(HashMap::new());
                for s in &block.body {
                    self.statement(f, s);
                }
                f.scopes.pop();
            }
            Statement::ExpressionStatement(e) => {
                self.expression(f, &e.expression, None, true);
            }
            Statement::VariableDeclaration(declaration) => self.declaration(f, declaration),
            Statement::IfStatement(s) => {
                let Some(cond) = self.boolean(f, &s.test) else {
                    return;
                };
                let branch = self.emit(
                    f,
                    Instr::Branch {
                        cond,
                        then: 0,
                        otherwise: 0,
                    },
                );
                Self::patch_branch(f, branch, Some(Self::here(f)), None);
                self.scoped(f, &s.consequent);
                match &s.alternate {
                    Some(alternate) => {
                        let over = self.emit(f, Instr::Jump { target: 0 });
                        Self::patch_branch(f, branch, None, Some(Self::here(f)));
                        self.scoped(f, alternate);
                        Self::patch(f, over, Self::here(f));
                    }
                    None => Self::patch_branch(f, branch, None, Some(Self::here(f))),
                }
            }
            Statement::WhileStatement(s) => {
                let top = Self::here(f);
                let Some(cond) = self.boolean(f, &s.test) else {
                    return;
                };
                let branch = self.emit(
                    f,
                    Instr::Branch {
                        cond,
                        then: 0,
                        otherwise: 0,
                    },
                );
                Self::patch_branch(f, branch, Some(Self::here(f)), None);
                f.loops.push(LoopCtx::default());
                self.scoped(f, &s.body);
                let ctx = f.loops.pop().expect("pushed above");
                let back = self.emit(f, Instr::Jump { target: top });
                let _ = back;
                let end = Self::here(f);
                Self::patch_branch(f, branch, None, Some(end));
                for at in ctx.breaks {
                    Self::patch(f, at, end);
                }
                for at in ctx.continues {
                    Self::patch(f, at, top);
                }
            }
            Statement::ForStatement(s) => {
                f.scopes.push(HashMap::new());
                match &s.init {
                    Some(ForStatementInit::VariableDeclaration(d)) => self.declaration(f, d),
                    Some(init) => {
                        if let Some(e) = init.as_expression() {
                            self.expression(f, e, None, true);
                        }
                    }
                    None => {}
                }
                let top = Self::here(f);
                let branch = match &s.test {
                    Some(test) => {
                        let Some(cond) = self.boolean(f, test) else {
                            f.scopes.pop();
                            return;
                        };
                        let at = self.emit(
                            f,
                            Instr::Branch {
                                cond,
                                then: 0,
                                otherwise: 0,
                            },
                        );
                        Self::patch_branch(f, at, Some(Self::here(f)), None);
                        Some(at)
                    }
                    None => None,
                };
                f.loops.push(LoopCtx::default());
                self.scoped(f, &s.body);
                let ctx = f.loops.pop().expect("pushed above");
                let update = Self::here(f);
                if let Some(update_expr) = &s.update {
                    self.at(f, update_expr.span());
                    self.expression(f, update_expr, None, true);
                }
                self.emit(f, Instr::Jump { target: top });
                let end = Self::here(f);
                if let Some(at) = branch {
                    Self::patch_branch(f, at, None, Some(end));
                }
                for at in ctx.breaks {
                    Self::patch(f, at, end);
                }
                for at in ctx.continues {
                    Self::patch(f, at, update);
                }
                f.scopes.pop();
            }
            Statement::BreakStatement(s) => {
                if s.label.is_some() {
                    self.unsupported(s.span, "labels");
                    return;
                }
                if f.loops.is_empty() {
                    self.err(s.span, "`break` outside a loop");
                    return;
                }
                let at = self.emit(f, Instr::Jump { target: 0 });
                f.loops.last_mut().expect("checked").breaks.push(at);
            }
            Statement::ContinueStatement(s) => {
                if s.label.is_some() {
                    self.unsupported(s.span, "labels");
                    return;
                }
                if f.loops.is_empty() {
                    self.err(s.span, "`continue` outside a loop");
                    return;
                }
                let at = self.emit(f, Instr::Jump { target: 0 });
                f.loops.last_mut().expect("checked").continues.push(at);
            }
            Statement::ReturnStatement(s) => match (&s.argument, f.ret.clone()) {
                (None, Type::Unit) => {
                    self.emit(f, Instr::Return { value: None });
                }
                (None, ret) => self.err(
                    s.span,
                    format!(
                        "this method returns `{}`",
                        crate::declarations::ts_type(&ret)
                    ),
                ),
                (Some(argument), Type::Unit) => {
                    self.err(argument.span(), "this method returns `void`")
                }
                (Some(argument), ret) => {
                    if let Some((reg, ty)) = self.expression(f, argument, Some(&ret), false) {
                        if ty == ret {
                            self.emit(f, Instr::Return { value: Some(reg) });
                        } else {
                            self.mismatch(argument.span(), &ret, &ty);
                        }
                    }
                }
            },
            Statement::DoWhileStatement(s) => self.unsupported(s.span, "`do ... while`"),
            Statement::ForInStatement(s) => self.unsupported(s.span, "`for ... in`"),
            Statement::ForOfStatement(s) => self.unsupported(s.span, "`for ... of`"),
            Statement::SwitchStatement(s) => self.unsupported(s.span, "`switch`"),
            Statement::TryStatement(s) => self.unsupported(s.span, "`try`"),
            Statement::ThrowStatement(s) => self.unsupported(s.span, "`throw`"),
            Statement::LabeledStatement(s) => self.unsupported(s.span, "labels"),
            other => self.unsupported(other.span(), "this statement"),
        }
    }

    fn scoped(&mut self, f: &mut Fx, statement: &Statement<'_>) {
        f.scopes.push(HashMap::new());
        self.statement(f, statement);
        f.scopes.pop();
    }

    fn declaration(&mut self, f: &mut Fx, declaration: &oxc_ast::ast::VariableDeclaration<'_>) {
        if declaration.kind == VariableDeclarationKind::Var {
            self.err(
                declaration.span,
                "use `let` or `const`: `var` is hoisted function-wide",
            );
            return;
        }
        for declarator in &declaration.declarations {
            let BindingPattern::BindingIdentifier(id) = &declarator.id else {
                self.unsupported(declarator.span, "destructuring");
                continue;
            };
            let annotated = self.annotation(&declarator.type_annotation);
            if declarator.type_annotation.is_some() && annotated.is_none() {
                continue;
            }
            let name = id.name.as_str().to_owned();
            if f.scopes.last().is_some_and(|s| s.contains_key(&name)) {
                self.err(
                    id.span,
                    format!("`{name}` is already declared in this scope"),
                );
                continue;
            }
            let (reg, ty) = match (&declarator.init, annotated) {
                (Some(init), annotated) => {
                    let Some((value, ty)) = self.expression(f, init, annotated.as_ref(), false)
                    else {
                        continue;
                    };
                    if let Some(a) = &annotated {
                        if *a != ty {
                            self.mismatch(init.span(), a, &ty);
                            continue;
                        }
                    }
                    if ty == Type::Unit {
                        self.err(init.span(), "a `void` value cannot be stored");
                        continue;
                    }
                    // A fresh register: the variable must not alias a value it was copied from.
                    let reg = self.new_reg(f, ty.clone());
                    self.emit(
                        f,
                        Instr::Move {
                            dst: reg,
                            src: value,
                        },
                    );
                    (reg, ty)
                }
                (None, Some(ty)) => (self.new_reg(f, ty.clone()), ty),
                (None, None) => {
                    self.err(
                        declarator.span,
                        format!("`{name}` needs a type annotation or an initializer"),
                    );
                    continue;
                }
            };
            f.scopes.last_mut().expect("a scope").insert(
                name,
                Local {
                    reg,
                    ty,
                    mutable: declaration.kind != VariableDeclarationKind::Const,
                },
            );
        }
    }

    fn mismatch(&mut self, span: Span, expected: &Type, found: &Type) {
        self.err(
            span,
            format!(
                "expected `{}`, found `{}`{}",
                crate::declarations::ts_type(expected),
                crate::declarations::ts_type(found),
                if matches!(
                    (expected, found),
                    (Type::Int, Type::Float) | (Type::Float, Type::Int)
                ) {
                    " (convert with `x as int` or `x as number`)"
                } else {
                    ""
                }
            ),
        );
    }

    fn boolean(&mut self, f: &mut Fx, expr: &Expression<'_>) -> Option<Reg> {
        let (reg, ty) = self.expression(f, expr, Some(&Type::Bool), false)?;
        if ty == Type::Bool {
            Some(reg)
        } else {
            self.mismatch(expr.span(), &Type::Bool, &ty);
            None
        }
    }

    // ---- expressions ----------------------------------------------------------------------

    /// Lower `expr`. `expected` steers numeric literals (an integer literal is a
    /// `number` where one is expected). `statement`: the value is unused.
    fn expression(
        &mut self,
        f: &mut Fx,
        expr: &Expression<'_>,
        expected: Option<&Type>,
        statement: bool,
    ) -> Option<Val> {
        match expr {
            Expression::ParenthesizedExpression(p) => {
                self.expression(f, &p.expression, expected, statement)
            }
            Expression::BooleanLiteral(b) => {
                Some(self.constant(f, Constant::Bool(b.value), Type::Bool))
            }
            Expression::StringLiteral(s) => {
                Some(self.constant(f, Constant::Str(s.value.as_str().to_owned()), Type::Str))
            }
            Expression::NumericLiteral(n) => {
                let (constant, ty) =
                    number_constant(n.value, n.raw.as_ref().map(|r| r.as_str()), expected);
                Some(self.constant(f, constant, ty))
            }
            Expression::Identifier(id) => match lookup(f, id.name.as_str()) {
                Some(local) => Some((local.reg, local.ty)),
                None => {
                    self.err(id.span, format!("unknown name `{}`", id.name));
                    None
                }
            },
            Expression::ThisExpression(t) => {
                self.err(
                    t.span,
                    "`this` can only be used to reach a field or method: `this.name`",
                );
                None
            }
            Expression::StaticMemberExpression(member) => self.member(f, member),
            Expression::CallExpression(call) => self.call(f, call, statement),
            Expression::AwaitExpression(a) => self.await_expression(f, a),
            Expression::UnaryExpression(u) => self.unary(f, u, expected),
            Expression::BinaryExpression(b) => self.binary(f, b, expected),
            Expression::LogicalExpression(l) => self.logical(f, l),
            Expression::ConditionalExpression(c) => self.conditional(f, c, expected),
            Expression::AssignmentExpression(a) => self.assignment(f, a),
            Expression::UpdateExpression(u) => self.update(f, u),
            Expression::TSAsExpression(a) => self.convert(f, a),
            Expression::TemplateLiteral(t) => {
                self.unsupported(
                    t.span,
                    "template literals: join strings with `+` and convert with `x as string`",
                );
                None
            }
            other => {
                self.unsupported(other.span(), "this expression");
                None
            }
        }
    }

    fn constant(&mut self, f: &mut Fx, constant: Constant, ty: Type) -> Val {
        let index = self.konst(constant);
        let dst = self.new_reg(f, ty.clone());
        self.emit(f, Instr::Const { dst, index });
        (dst, ty)
    }

    /// `this.name` read, or a getter native on a value or component.
    fn member(
        &mut self,
        f: &mut Fx,
        member: &oxc_ast::ast::StaticMemberExpression<'_>,
    ) -> Option<Val> {
        let name = member.property.name.as_str();
        if matches!(member.object, Expression::ThisExpression(_)) {
            if let Some((var, ty)) = self.fields.get(name).cloned() {
                let dst = self.new_reg(f, ty.clone());
                self.emit(f, Instr::LoadVar { dst, var });
                return Some((dst, ty));
            }
            return match name {
                "entity" => {
                    let dst = self.new_reg(f, Type::Entity);
                    self.emit(f, Instr::SelfEntity { dst });
                    Some((dst, Type::Entity))
                }
                "time" => {
                    let dst = self.new_reg(f, Type::Float);
                    self.emit(f, Instr::Now { dst });
                    Some((dst, Type::Float))
                }
                _ if self.methods.contains_key(name) => {
                    self.err(member.span, format!("`this.{name}` is a method: call it"));
                    None
                }
                _ => {
                    self.err(member.span, format!("the class has no field `{name}`"));
                    None
                }
            };
        }
        let (receiver, ty) = self.expression(f, &member.object, None, false)?;
        let receiver_type = match &ty {
            Type::Component(t) | Type::Object(t) => t.clone(),
            other => {
                self.err(
                    member.span,
                    format!(
                        "`{}` has no property `{name}`",
                        crate::declarations::ts_type(other)
                    ),
                );
                return None;
            }
        };
        let Some(native) = self
            .native(&receiver_type, name)
            .filter(|n| n.sig.params.len() == 1 && n.sig.ret != Type::Unit)
        else {
            self.err(
                member.span,
                format!("`{receiver_type}` has no property `{name}`"),
            );
            return None;
        };
        let import = self.import(&native.name)?;
        let ret = native.sig.ret.clone();
        let dst = self.new_reg(f, ret.clone());
        self.emit(
            f,
            Instr::CallNative {
                import,
                args: vec![receiver],
                dst: Some(dst),
            },
        );
        Some((dst, ret))
    }

    fn call(
        &mut self,
        f: &mut Fx,
        call: &oxc_ast::ast::CallExpression<'_>,
        statement: bool,
    ) -> Option<Val> {
        self.at(f, call.span);
        if call.optional || call.type_arguments.is_some() {
            self.unsupported(call.span, "optional chaining and explicit type arguments");
            return None;
        }
        let args: Vec<&Expression<'_>> = call
            .arguments
            .iter()
            .filter_map(Argument::as_expression)
            .collect();
        if args.len() != call.arguments.len() {
            self.unsupported(call.span, "spread arguments");
            return None;
        }
        match &call.callee {
            Expression::Identifier(id) if id.name.as_str() == "wait" => {
                self.err(
                    call.span,
                    "`wait(..)` must be awaited: `await wait(seconds)` in an `async` method",
                );
                None
            }
            Expression::StaticMemberExpression(member) => {
                let name = member.property.name.as_str();
                // this.method(..): a call to a method of the class.
                if matches!(member.object, Expression::ThisExpression(_)) {
                    return self.call_method(f, call.span, name, &args, false);
                }
                // ns.fn(..): a native; value.fn(..): a native with the value as receiver.
                if let Expression::Identifier(ns) = &member.object {
                    if lookup(f, ns.name.as_str()).is_none() {
                        let Some(native) = self.native(ns.name.as_str(), name) else {
                            self.err(call.span, format!("no native `{}.{name}`", ns.name));
                            return None;
                        };
                        return self.call_native(f, call.span, &native, None, &args, statement);
                    }
                }
                let (receiver, ty) = self.expression(f, &member.object, None, false)?;
                let receiver_type = match &ty {
                    Type::Component(t) | Type::Object(t) => t.clone(),
                    other => {
                        self.err(
                            call.span,
                            format!(
                                "`{}` has no method `{name}`",
                                crate::declarations::ts_type(other)
                            ),
                        );
                        return None;
                    }
                };
                let Some(native) = self.native(&receiver_type, name) else {
                    self.err(
                        call.span,
                        format!("`{receiver_type}` has no method `{name}`"),
                    );
                    return None;
                };
                self.call_native(
                    f,
                    call.span,
                    &native,
                    Some((receiver, ty)),
                    &args,
                    statement,
                )
            }
            other => {
                self.unsupported(other.span(), "calling this expression");
                None
            }
        }
    }

    fn call_method(
        &mut self,
        f: &mut Fx,
        span: Span,
        name: &str,
        args: &[&Expression<'_>],
        awaited: bool,
    ) -> Option<Val> {
        let Some(sig) = self.methods.get(name).cloned() else {
            self.err(span, format!("the class has no method `{name}`"));
            return None;
        };
        if sig.is_async && !awaited {
            self.err(
                span,
                format!("`{name}` is async: call it as `await this.{name}(..)`"),
            );
            return None;
        }
        if !sig.is_async && awaited {
            self.err(span, format!("`{name}` is not async: remove the `await`"));
            return None;
        }
        if sig.is_async && !f.is_async {
            self.err(span, "only an `async` method can await");
            return None;
        }
        let regs = self.arguments(f, span, name, &sig.params, args)?;
        let dst = (sig.ret != Type::Unit).then(|| self.new_reg(f, sig.ret.clone()));
        self.emit(
            f,
            Instr::Call {
                func: sig.index,
                args: regs,
                dst,
            },
        );
        Some((dst.unwrap_or(0), sig.ret))
    }

    /// Lower `args` against `params`.
    fn arguments(
        &mut self,
        f: &mut Fx,
        span: Span,
        what: &str,
        params: &[Type],
        args: &[&Expression<'_>],
    ) -> Option<Vec<Reg>> {
        if args.len() != params.len() {
            self.err(
                span,
                format!(
                    "`{what}` takes {} argument(s), got {}",
                    params.len(),
                    args.len()
                ),
            );
            return None;
        }
        let mut regs = Vec::new();
        let mut ok = true;
        for (arg, param) in args.iter().zip(params) {
            match self.expression(f, arg, Some(param), false) {
                Some((reg, ty)) if ty == *param => regs.push(reg),
                Some((_, ty)) => {
                    self.mismatch(arg.span(), param, &ty);
                    ok = false;
                }
                None => ok = false,
            }
        }
        ok.then_some(regs)
    }

    fn call_native(
        &mut self,
        f: &mut Fx,
        span: Span,
        native: &pulsar_script_vm::NativeFn,
        receiver: Option<Val>,
        args: &[&Expression<'_>],
        _statement: bool,
    ) -> Option<Val> {
        let mut params: &[Param] = &native.sig.params;
        let mut regs = Vec::new();
        if let Some((reg, ty)) = receiver {
            let Some(first) = params.first() else {
                self.err(span, format!("`{}` takes no receiver", native.name));
                return None;
            };
            if first.ty != ty {
                self.err(
                    span,
                    format!(
                        "`{}` needs a `{}` receiver",
                        native.name,
                        crate::declarations::ts_type(&first.ty)
                    ),
                );
                return None;
            }
            regs.push(reg);
            params = &params[1..];
        }
        let types: Vec<Type> = params.iter().map(|p| p.ty.clone()).collect();
        let rest = self.arguments(f, span, &native.name, &types, args)?;
        regs.extend(rest);
        let import = self.import(&native.name)?;
        let ret = native.sig.ret.clone();
        let dst = (ret != Type::Unit).then(|| self.new_reg(f, ret.clone()));
        self.emit(
            f,
            Instr::CallNative {
                import,
                args: regs,
                dst,
            },
        );
        Some((dst.unwrap_or(0), ret))
    }

    fn await_expression(
        &mut self,
        f: &mut Fx,
        a: &oxc_ast::ast::AwaitExpression<'_>,
    ) -> Option<Val> {
        if !f.is_async {
            self.err(a.span, "`await` is only allowed in an `async` method");
            return None;
        }
        let Expression::CallExpression(call) = &a.argument else {
            self.err(
                a.span,
                "only `await wait(seconds)` and `await this.method()` are supported",
            );
            return None;
        };
        let args: Vec<&Expression<'_>> = call
            .arguments
            .iter()
            .filter_map(Argument::as_expression)
            .collect();
        match &call.callee {
            Expression::Identifier(id) if id.name.as_str() == "wait" => {
                if args.len() != 1 {
                    self.err(call.span, "`wait` takes one argument: seconds");
                    return None;
                }
                let regs = self.arguments(f, call.span, "wait", &[Type::Float], &args)?;
                self.at(f, a.span);
                self.emit(f, Instr::Wait { seconds: regs[0] });
                Some((0, Type::Unit))
            }
            Expression::StaticMemberExpression(member)
                if matches!(member.object, Expression::ThisExpression(_)) =>
            {
                self.at(f, a.span);
                self.call_method(f, call.span, member.property.name.as_str(), &args, true)
            }
            _ => {
                self.err(
                    a.span,
                    "only `await wait(seconds)` and `await this.method()` are supported",
                );
                None
            }
        }
    }

    fn unary(
        &mut self,
        f: &mut Fx,
        u: &oxc_ast::ast::UnaryExpression<'_>,
        expected: Option<&Type>,
    ) -> Option<Val> {
        match u.operator {
            UnaryOperator::UnaryNegation => {
                if let Expression::NumericLiteral(n) = &u.argument {
                    let (constant, ty) =
                        number_constant(-n.value, n.raw.as_ref().map(|r| r.as_str()), expected);
                    return Some(self.constant(f, constant, ty));
                }
                let (reg, ty) = self.expression(f, &u.argument, expected, false)?;
                if !ty.is_numeric() {
                    self.err(u.span, "unary `-` needs a number");
                    return None;
                }
                let dst = self.new_reg(f, ty.clone());
                self.emit(
                    f,
                    Instr::Unary {
                        op: UnOp::Neg,
                        dst,
                        src: reg,
                    },
                );
                Some((dst, ty))
            }
            UnaryOperator::UnaryPlus => self.expression(f, &u.argument, expected, false),
            UnaryOperator::LogicalNot => {
                let reg = self.boolean(f, &u.argument)?;
                let dst = self.new_reg(f, Type::Bool);
                self.emit(
                    f,
                    Instr::Unary {
                        op: UnOp::Not,
                        dst,
                        src: reg,
                    },
                );
                Some((dst, Type::Bool))
            }
            _ => {
                self.unsupported(u.span, "this operator");
                None
            }
        }
    }

    /// Both operands, steering a numeric literal by the other side's type.
    fn operands(
        &mut self,
        f: &mut Fx,
        left: &Expression<'_>,
        right: &Expression<'_>,
        hint: Option<&Type>,
    ) -> Option<(Val, Val)> {
        if is_number_literal(left) && !is_number_literal(right) {
            let r = self.expression(f, right, hint, false)?;
            let l = self.expression(f, left, Some(&r.1), false)?;
            return Some((l, r));
        }
        let l = self.expression(f, left, hint, false)?;
        let hint = if is_number_literal(right) {
            Some(&l.1)
        } else {
            None
        };
        let r = self.expression(f, right, hint, false)?;
        Some((l, r))
    }

    fn binary(
        &mut self,
        f: &mut Fx,
        b: &oxc_ast::ast::BinaryExpression<'_>,
        expected: Option<&Type>,
    ) -> Option<Val> {
        let arithmetic = matches!(
            b.operator,
            BinaryOperator::Addition
                | BinaryOperator::Subtraction
                | BinaryOperator::Multiplication
                | BinaryOperator::Division
                | BinaryOperator::Remainder
        );
        let hint = if arithmetic {
            expected.filter(|t| t.is_numeric())
        } else {
            None
        };
        let (left, right) = self.operands(f, &b.left, &b.right, hint)?;
        self.at(f, b.span);
        let op = match b.operator {
            BinaryOperator::Addition => BinOp::Add,
            BinaryOperator::Subtraction => BinOp::Sub,
            BinaryOperator::Multiplication => BinOp::Mul,
            BinaryOperator::Division => BinOp::Div,
            BinaryOperator::Remainder => BinOp::Rem,
            BinaryOperator::LessThan => BinOp::Lt,
            BinaryOperator::LessEqualThan => BinOp::Le,
            BinaryOperator::GreaterThan => BinOp::Gt,
            BinaryOperator::GreaterEqualThan => BinOp::Ge,
            BinaryOperator::Equality | BinaryOperator::StrictEquality => BinOp::Eq,
            BinaryOperator::Inequality | BinaryOperator::StrictInequality => BinOp::Ne,
            _ => {
                self.unsupported(b.span, "this operator");
                return None;
            }
        };
        self.binary_op(f, b.span, op, left, right)
    }

    fn binary_op(
        &mut self,
        f: &mut Fx,
        span: Span,
        op: BinOp,
        left: Val,
        right: Val,
    ) -> Option<Val> {
        if left.1 != right.1 {
            self.err(
                span,
                format!(
                    "the operands have different types (`{}` and `{}`){}",
                    crate::declarations::ts_type(&left.1),
                    crate::declarations::ts_type(&right.1),
                    if left.1.is_numeric() && right.1.is_numeric() {
                        ": convert one with `x as number` or `x as int`"
                    } else {
                        ""
                    }
                ),
            );
            return None;
        }
        let ty = left.1.clone();
        let result = match op {
            BinOp::Add if matches!(ty, Type::Int | Type::Float | Type::Str) => ty.clone(),
            BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem if ty.is_numeric() => ty.clone(),
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                if matches!(ty, Type::Int | Type::Float | Type::Str) =>
            {
                Type::Bool
            }
            BinOp::Eq | BinOp::Ne
                if matches!(
                    ty,
                    Type::Int
                        | Type::Float
                        | Type::Str
                        | Type::Bool
                        | Type::Entity
                        | Type::Component(_)
                ) || pulsar_script_vm::TypeRegistry::global().supports_eq(&ty) =>
            {
                Type::Bool
            }
            _ => {
                self.err(
                    span,
                    format!(
                        "this operator does not apply to `{}`{}",
                        crate::declarations::ts_type(&ty),
                        if matches!(ty, Type::Object(_)) {
                            ": use the type's methods (`a.add(b)`, `a.eq(b)`)"
                        } else {
                            ""
                        }
                    ),
                );
                return None;
            }
        };
        let dst = self.new_reg(f, result.clone());
        self.emit(
            f,
            Instr::Binary {
                op,
                dst,
                a: left.0,
                b: right.0,
            },
        );
        Some((dst, result))
    }

    fn logical(&mut self, f: &mut Fx, l: &oxc_ast::ast::LogicalExpression<'_>) -> Option<Val> {
        if l.operator == LogicalOperator::Coalesce {
            self.unsupported(l.span, "`??`");
            return None;
        }
        let left = self.boolean(f, &l.left)?;
        let result = self.new_reg(f, Type::Bool);
        self.emit(
            f,
            Instr::Move {
                dst: result,
                src: left,
            },
        );
        let branch = self.emit(
            f,
            Instr::Branch {
                cond: left,
                then: 0,
                otherwise: 0,
            },
        );
        // `&&` evaluates the right side when the left is true, `||` when false.
        let right_start = Self::here(f);
        let right = self.boolean(f, &l.right)?;
        self.emit(
            f,
            Instr::Move {
                dst: result,
                src: right,
            },
        );
        let end = Self::here(f);
        match l.operator {
            LogicalOperator::And => Self::patch_branch(f, branch, Some(right_start), Some(end)),
            _ => Self::patch_branch(f, branch, Some(end), Some(right_start)),
        }
        Some((result, Type::Bool))
    }

    fn conditional(
        &mut self,
        f: &mut Fx,
        c: &oxc_ast::ast::ConditionalExpression<'_>,
        expected: Option<&Type>,
    ) -> Option<Val> {
        let cond = self.boolean(f, &c.test)?;
        let branch = self.emit(
            f,
            Instr::Branch {
                cond,
                then: 0,
                otherwise: 0,
            },
        );
        Self::patch_branch(f, branch, Some(Self::here(f)), None);
        let (a, ty) = self.expression(f, &c.consequent, expected, false)?;
        let result = self.new_reg(f, ty.clone());
        self.emit(
            f,
            Instr::Move {
                dst: result,
                src: a,
            },
        );
        let over = self.emit(f, Instr::Jump { target: 0 });
        Self::patch_branch(f, branch, None, Some(Self::here(f)));
        let (b, other) = self.expression(f, &c.alternate, Some(&ty), false)?;
        if other != ty {
            self.mismatch(c.alternate.span(), &ty, &other);
            return None;
        }
        self.emit(
            f,
            Instr::Move {
                dst: result,
                src: b,
            },
        );
        Self::patch(f, over, Self::here(f));
        Some((result, ty))
    }

    /// `x as int`, `x as number`, `x as string`.
    fn convert(&mut self, f: &mut Fx, a: &oxc_ast::ast::TSAsExpression<'_>) -> Option<Val> {
        let target = self.resolve_type(&a.type_annotation)?;
        let (reg, ty) = self.expression(f, &a.expression, Some(&target), false)?;
        if ty == target {
            return Some((reg, ty));
        }
        let op = match (&ty, &target) {
            (Type::Int, Type::Float) => UnOp::IntToFloat,
            (Type::Float, Type::Int) => UnOp::FloatToInt,
            (Type::Int | Type::Float | Type::Bool, Type::Str) => UnOp::ToStr,
            _ => {
                self.err(
                    a.span,
                    format!(
                        "cannot convert `{}` to `{}`",
                        crate::declarations::ts_type(&ty),
                        crate::declarations::ts_type(&target)
                    ),
                );
                return None;
            }
        };
        let dst = self.new_reg(f, target.clone());
        self.emit(f, Instr::Unary { op, dst, src: reg });
        Some((dst, target))
    }

    // ---- assignment ------------------------------------------------------------------------

    fn assignment(
        &mut self,
        f: &mut Fx,
        a: &oxc_ast::ast::AssignmentExpression<'_>,
    ) -> Option<Val> {
        let target = self.target(f, &a.left)?;
        let current = target.ty().clone();
        let value = match a.operator {
            AssignmentOperator::Assign => {
                let (reg, ty) = self.expression(f, &a.right, Some(&current), false)?;
                if ty != current {
                    self.mismatch(a.right.span(), &current, &ty);
                    return None;
                }
                (reg, ty)
            }
            op => {
                let bin = match op {
                    AssignmentOperator::Addition => BinOp::Add,
                    AssignmentOperator::Subtraction => BinOp::Sub,
                    AssignmentOperator::Multiplication => BinOp::Mul,
                    AssignmentOperator::Division => BinOp::Div,
                    AssignmentOperator::Remainder => BinOp::Rem,
                    _ => {
                        self.unsupported(a.span, "this assignment operator");
                        return None;
                    }
                };
                let old = self.read_target(f, &target);
                let right = self.expression(f, &a.right, Some(&current), false)?;
                self.binary_op(f, a.span, bin, old, right)?
            }
        };
        if value.1 != current {
            self.mismatch(a.span, &current, &value.1);
            return None;
        }
        self.at(f, a.span);
        self.write_target(f, &target, value.0);
        Some(value)
    }

    fn update(&mut self, f: &mut Fx, u: &oxc_ast::ast::UpdateExpression<'_>) -> Option<Val> {
        let target = self.simple_target(f, &u.argument)?;
        let ty = target.ty().clone();
        if !ty.is_numeric() {
            self.err(u.span, "`++` and `--` need a number");
            return None;
        }
        let old = self.read_target(f, &target);
        let one = match ty {
            Type::Int => self.constant(f, Constant::Int(1), Type::Int),
            _ => self.constant(f, Constant::Float(1.0), Type::Float),
        };
        let op = if u.operator == UpdateOperator::Increment {
            BinOp::Add
        } else {
            BinOp::Sub
        };
        let new = self.binary_op(f, u.span, op, old.clone(), one)?;
        // A postfix update yields the old value, so keep it in its own register.
        let previous = if u.prefix {
            None
        } else {
            let keep = self.new_reg(f, ty.clone());
            self.emit(
                f,
                Instr::Move {
                    dst: keep,
                    src: old.0,
                },
            );
            Some(keep)
        };
        self.write_target(f, &target, new.0);
        Some((previous.unwrap_or(new.0), ty))
    }

    fn target(&mut self, f: &mut Fx, target: &AssignmentTarget<'_>) -> Option<Target> {
        match target.as_simple_assignment_target() {
            Some(simple) => self.simple_target(f, simple),
            None => {
                self.unsupported(target.span(), "destructuring assignment");
                None
            }
        }
    }

    fn simple_target(&mut self, f: &mut Fx, target: &SimpleAssignmentTarget<'_>) -> Option<Target> {
        match target {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(id) => {
                match lookup(f, id.name.as_str()) {
                    Some(local) if local.mutable => Some(Target::Local(local.reg, local.ty)),
                    Some(_) => {
                        self.err(id.span, format!("`{}` is a `const`", id.name));
                        None
                    }
                    None => {
                        self.err(id.span, format!("unknown name `{}`", id.name));
                        None
                    }
                }
            }
            SimpleAssignmentTarget::StaticMemberExpression(member)
                if matches!(member.object, Expression::ThisExpression(_)) =>
            {
                let name = member.property.name.as_str();
                match self.fields.get(name) {
                    Some((var, ty)) => Some(Target::Field(*var, ty.clone())),
                    None => {
                        self.err(member.span, format!("the class has no field `{name}`"));
                        None
                    }
                }
            }
            SimpleAssignmentTarget::StaticMemberExpression(member) => {
                self.err(member.span, "only the class's own fields can be assigned: replace a part of a value with its `with_..` method");
                None
            }
            other => {
                self.unsupported(other.span(), "this assignment target");
                None
            }
        }
    }

    fn read_target(&mut self, f: &mut Fx, target: &Target) -> Val {
        match target {
            Target::Local(reg, ty) => (*reg, ty.clone()),
            Target::Field(var, ty) => {
                let dst = self.new_reg(f, ty.clone());
                self.emit(f, Instr::LoadVar { dst, var: *var });
                (dst, ty.clone())
            }
        }
    }

    fn write_target(&mut self, f: &mut Fx, target: &Target, src: Reg) {
        match target {
            Target::Local(reg, _) => {
                if *reg != src {
                    self.emit(f, Instr::Move { dst: *reg, src });
                }
            }
            Target::Field(var, _) => {
                self.emit(f, Instr::StoreVar { var: *var, src });
            }
        }
    }
}

enum Target {
    Local(Reg, Type),
    Field(u32, Type),
}

impl Target {
    fn ty(&self) -> &Type {
        match self {
            Self::Local(_, ty) | Self::Field(_, ty) => ty,
        }
    }
}

fn lookup(f: &Fx, name: &str) -> Option<Local> {
    f.scopes.iter().rev().find_map(|s| s.get(name)).cloned()
}

fn is_number_literal(e: &Expression<'_>) -> bool {
    numeric_literal(e).is_some()
}

/// The value of a number literal, possibly negated or parenthesized.
fn numeric_literal(e: &Expression<'_>) -> Option<f64> {
    match e {
        Expression::NumericLiteral(n) => Some(n.value),
        Expression::ParenthesizedExpression(p) => numeric_literal(&p.expression),
        Expression::UnaryExpression(u) if u.operator == UnaryOperator::UnaryNegation => {
            numeric_literal(&u.argument).map(|v| -v)
        }
        _ => None,
    }
}

/// A number literal as a constant: an integer literal is an `int` unless a
/// `number` is expected; anything with a fraction or exponent is a `number`.
fn number_constant(value: f64, raw: Option<&str>, expected: Option<&Type>) -> (Constant, Type) {
    let integral_syntax = raw.is_none_or(|r| {
        let r = r.to_ascii_lowercase();
        r.starts_with("0x") || r.starts_with("0b") || r.starts_with("0o") || !r.contains(['.', 'e'])
    });
    let as_int = integral_syntax
        && value.fract() == 0.0
        && value.abs() < 9.2e18
        && !matches!(expected, Some(Type::Float));
    if as_int {
        (Constant::Int(value as i64), Type::Int)
    } else {
        (Constant::Float(value), Type::Float)
    }
}

/// `@renamedFrom("old")` -> `old`.
fn renamed_from_decorator(expression: &Expression<'_>) -> Option<String> {
    let Expression::CallExpression(call) = expression else {
        return None;
    };
    let Expression::Identifier(callee) = &call.callee else {
        return None;
    };
    if callee.name.as_str() != "renamedFrom" || call.arguments.len() != 1 {
        return None;
    }
    match call.arguments[0].as_expression()? {
        Expression::StringLiteral(s) => Some(s.value.as_str().to_owned()),
        _ => None,
    }
}

/// Whether control cannot continue past `statement`.
fn terminates(statement: &Statement<'_>) -> bool {
    match statement {
        Statement::ReturnStatement(_) => true,
        Statement::BlockStatement(b) => b.body.iter().any(terminates),
        Statement::IfStatement(i) => i
            .alternate
            .as_ref()
            .is_some_and(|alt| terminates(&i.consequent) && terminates(alt)),
        _ => false,
    }
}
