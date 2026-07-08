use smallvec::SmallVec;
use ty_python_core::definition::{Definition, DefinitionState};
use ty_python_core::scope::ScopeId;
use ty_python_core::{
    BindingWithConstraintsIterator, ProgramFile, global_scope, place_table, use_def_map,
};

use crate::Db;
use crate::place::{
    Place, builtins_module_scope, class_body_implicit_symbol, implicit_builtins_symbol,
    module_type_implicit_global_symbol,
};
use crate::place_load::{ImplicitPlaceLoad, PlaceLoadSource, PlaceLoadSourceKind};
use crate::reachability::ReachabilityConstraintsExtension;
use crate::types::ProgramEnvironment;

/// Returns the definitions that may supply a module global at the end of its scope.
pub(crate) fn definitions_for_module_global<'db>(
    db: &'db dyn Db,
    file: ProgramFile<'db>,
    name: &str,
) -> Option<DefinitionResolution<'db>> {
    let scope = global_scope(db, file);
    let symbol = place_table(db, scope).symbol_id(name)?;
    Some(DefinitionResolution::from_bindings(
        db,
        use_def_map(db, scope).end_of_scope_symbol_bindings(symbol),
    ))
}

/// The definitions that may bind a name and information about the resolution.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DefinitionResolution<'db> {
    /// Definitions that may bind the name.
    definitions: SmallVec<[Definition<'db>; 2]>,
    /// Whether lookup crosses a `global` or `nonlocal` declaration.
    pub(crate) crosses_scope_declaration: bool,
    /// Whether the definitions include a source definition for every possible binding.
    pub(crate) is_complete: bool,
}

impl<'db> DefinitionResolution<'db> {
    pub(crate) fn new() -> Self {
        Self {
            definitions: SmallVec::new(),
            crosses_scope_declaration: false,
            is_complete: true,
        }
    }

    /// Collects reachable definitions.
    pub(crate) fn from_bindings(
        db: &'db dyn Db,
        mut bindings: BindingWithConstraintsIterator<'db, 'db>,
    ) -> Self {
        let mut resolution = Self::new();

        while let Some(binding) = bindings.next() {
            if let DefinitionState::Defined(definition) = binding.binding
                && !bindings
                    .reachability_constraints()
                    .evaluate(db, bindings.predicates(), binding.reachability_constraint)
                    .is_always_false()
            {
                resolution.push_definition(db, definition);
            }
        }

        resolution
    }

    fn from_place_load_source(
        db: &'db dyn Db,
        environment: &ProgramEnvironment<'db>,
        scope: ScopeId<'db>,
        source: &PlaceLoadSource<'db>,
    ) -> Self {
        match &source.kind {
            PlaceLoadSourceKind::Bindings(bindings) => Self::from_bindings(db, bindings.clone()),
            PlaceLoadSourceKind::DefinitionsFromOwningScope { scope, id } => {
                Self::from_bindings(db, use_def_map(db, *scope).reachable_bindings(*id))
            }
            PlaceLoadSourceKind::Implicit(ImplicitPlaceLoad::ExplicitGlobalSymbol {
                file,
                name,
            }) => {
                let scope = global_scope(db, *file);
                let Some(symbol) = place_table(db, scope).symbol_id(name) else {
                    return Self::new();
                };
                Self::from_bindings(db, use_def_map(db, scope).reachable_symbol_bindings(symbol))
            }
            PlaceLoadSourceKind::Implicit(ImplicitPlaceLoad::DunderClass(class_def)) => {
                let mut resolution = Self::new();
                resolution.push_definition(db, *class_def);
                resolution
            }
            PlaceLoadSourceKind::Implicit(ImplicitPlaceLoad::ClassBodySymbol(name)) => {
                Self::from_place_without_definition(
                    class_body_implicit_symbol(db, environment, name).place,
                )
            }
            PlaceLoadSourceKind::Implicit(ImplicitPlaceLoad::ModuleImplicitGlobal {
                file,
                name,
            }) => Self::from_place_without_definition(
                module_type_implicit_global_symbol(db, *file, name).place,
            ),
            PlaceLoadSourceKind::Implicit(ImplicitPlaceLoad::Builtin(name)) => {
                Self::from_builtin(db, environment, scope, name)
            }
        }
    }

    fn from_builtin(
        db: &'db dyn Db,
        environment: &ProgramEnvironment<'db>,
        scope: ScopeId<'db>,
        name: &str,
    ) -> Self {
        if Some(scope) == builtins_module_scope(db, environment) {
            // Skip builtin fallback in the `builtins` global scope to avoid a recursive lookup.
            return Self::from_place_without_definition(Place::Undefined);
        }

        // Builtin values have no explicit import for a consumer to follow. Record whether a
        // value exists without collecting definitions that consumers cannot use.
        Self::from_place_without_definition(implicit_builtins_symbol(db, environment, name).place)
    }

    fn from_place_without_definition(place: Place<'db>) -> Self {
        Self {
            is_complete: place.is_undefined(),
            ..Self::new()
        }
    }

    pub(crate) fn add_source(
        &mut self,
        db: &'db dyn Db,
        environment: &ProgramEnvironment<'db>,
        scope: ScopeId<'db>,
        source: &PlaceLoadSource<'db>,
    ) {
        self.extend(
            db,
            Self::from_place_load_source(db, environment, scope, source),
        );
    }

    fn push_definition(&mut self, db: &'db dyn Db, definition: Definition<'db>) {
        self.is_complete &= definition.kind(db).is_user_visible();
        if !self.definitions.contains(&definition) {
            self.definitions.push(definition);
        }
    }

    fn extend(&mut self, db: &'db dyn Db, other: Self) {
        for definition in other.definitions {
            self.push_definition(db, definition);
        }
        self.is_complete &= other.is_complete;
    }

    /// Returns the definitions that may bind the name.
    ///
    /// This can include synthetic definitions, such as loop headers.
    pub fn definitions(&self) -> &[Definition<'db>] {
        &self.definitions
    }

    /// Returns whether lookup crosses a `global` or `nonlocal` declaration.
    pub fn crosses_scope_declaration(&self) -> bool {
        self.crosses_scope_declaration
    }

    /// Returns whether the definitions include a source definition for every possible binding.
    ///
    /// For example, implicit builtin values are incomplete because no explicit import connects the
    /// name to their definitions.
    ///
    /// Note that a complete resolution can still leave a name possibly unbound.
    pub fn is_complete(&self) -> bool {
        self.is_complete
    }
}

#[cfg(test)]
mod tests {
    use ruff_db::files::system_path_to_file;
    use ruff_db::parsed::parsed_module;
    use ruff_python_ast::AnyNodeRef;
    use ruff_python_ast::find_node::covering_node;
    use ruff_python_trivia::textwrap::dedent;
    use ruff_text_size::{Ranged, TextRange, TextSize};
    use ty_python_core::ProgramFile;
    use ty_python_core::place::PlaceExpr;
    use ty_python_core::semantic_index;

    use super::*;
    use crate::db::tests::{TestDb, TestDbBuilder};
    use crate::place_load::{PlaceLoadMode, PlaceLoadResolutionStep, resolve_place_load};

    #[test]
    fn resolution_collects_reachable_definitions_after_conditional_deletion() {
        let test = ResolutionTest::new(
            "
            value = 1
            if condition:
                value = 2
            if condition:
                del value
            read = value
            ",
        );

        let resolution = test.first_source_for("read = [value]");

        assert_eq!(
            test.definition_texts(&resolution),
            ["value = 1", "value = 2"]
        );
        assert!(resolution.is_complete());
    }

    #[test]
    fn merging_resolutions_combines_and_deduplicates_definitions() {
        let test = ResolutionTest::new(
            "
            value = 1
            first_read = value
            value = 2
            second_read = value
            ",
        );

        let mut merged = test.first_source_for("first_read = [value]");
        let other = test.first_source_for("second_read = [value]");
        let mut expected = merged.definitions().to_vec();
        expected.extend_from_slice(other.definitions());
        merged.extend(&test.db, other);
        merged.extend(&test.db, merged.clone());

        assert_eq!(merged.definitions(), expected);
        assert_eq!(test.definition_texts(&merged), ["value = 1", "value = 2"]);
        assert!(merged.is_complete());
        assert!(!merged.crosses_scope_declaration());
    }

    #[test]
    fn builtin_resolution_marks_existing_builtins_incomplete() {
        let test = ResolutionTest::new(
            "
            value = 1
            read = value
            ",
        );

        let mut resolution = test.first_source_for("read = [value]");
        let missing = test.builtin("not_a_builtin");
        let builtin = test.builtin("print");

        assert_eq!(test.definition_texts(&resolution), ["value = 1"]);
        assert!(resolution.is_complete());
        assert!(missing.is_complete());
        assert!(!builtin.is_complete());
        resolution.extend(&test.db, builtin);
        assert!(!resolution.is_complete());
    }

    #[test]
    fn resolution_retains_loop_headers_and_marks_them_incomplete() {
        let test = ResolutionTest::new(
            "
            value = 1
            while condition:
                loop_read = value
                value = 2
            ",
        );

        let source_resolution = test.first_source_for("loop_read = [value]");
        let scope = global_scope(&test.db, test.file());
        let symbol = place_table(&test.db, scope)
            .symbol_id("value")
            .expect("value has a symbol");
        let binding_resolution = DefinitionResolution::from_bindings(
            &test.db,
            use_def_map(&test.db, scope).end_of_scope_symbol_bindings(symbol),
        );

        for resolution in [source_resolution, binding_resolution] {
            assert!(
                resolution
                    .definitions()
                    .iter()
                    .any(|definition| definition.kind(&test.db).is_loop_header())
            );
            assert!(!resolution.is_complete());
        }
    }

    struct ResolutionTest {
        db: TestDb,
        source: String,
    }

    impl ResolutionTest {
        fn new(source: &str) -> Self {
            let source = dedent(source).into_owned();
            let db = TestDbBuilder::new()
                .with_file("/src/test.py", &source)
                .build()
                .expect("valid resolution test setup");
            Self { db, source }
        }

        /// Resolves the first source for a module-scope name. Brackets mark the name, as in
        /// `read = [value]`; the unbracketed text must occur exactly once in the source.
        #[track_caller]
        fn first_source_for(&self, selector: &str) -> DefinitionResolution<'_> {
            let (prefix, rest) = selector
                .split_once('[')
                .expect("selector needs `[` and `]`");
            let (name, suffix) = rest.split_once(']').expect("selector needs `[` and `]`");
            assert!(
                !prefix.contains(']')
                    && !name.is_empty()
                    && !name.contains('[')
                    && !suffix.contains(['[', ']']),
                "selector must contain one bracketed name: {selector:?}"
            );
            let unmarked = format!("{prefix}{name}{suffix}");
            let offset = self
                .source
                .find(&unmarked)
                .unwrap_or_else(|| panic!("selector not found in source: {selector:?}"));
            assert_eq!(
                self.source.rfind(&unmarked),
                Some(offset),
                "selector matches multiple occurrences: {selector:?}"
            );
            let start = TextSize::try_from(offset + prefix.len()).expect("source fits in TextSize");
            let end = start + TextSize::try_from(name.len()).expect("name fits in TextSize");
            let range = TextRange::new(start, end);

            let db = &self.db;
            let file = self.file();
            let module = parsed_module(db, file.python_file(db)).load(db);
            let node = covering_node(module.syntax().into(), range).node();
            let AnyNodeRef::ExprName(name) = node else {
                panic!("selector does not identify a name: {selector:?}");
            };
            assert!(
                name.range() == range && name.ctx.is_load(),
                "selector must identify an entire name load: {selector:?}"
            );

            let scope = global_scope(db, file);
            let environment = ProgramEnvironment::from_file(file);
            let mut sources = resolve_place_load(
                db,
                semantic_index(db, file),
                scope,
                PlaceExpr::from_expr_name(name),
                PlaceLoadMode::AtExpression(name.into()),
            );
            let Some(PlaceLoadResolutionStep::Source(source)) = sources.next() else {
                panic!("expected a first source for {selector:?}");
            };
            let mut resolution = DefinitionResolution::new();
            resolution.add_source(db, &environment, scope, &source);
            resolution
        }

        fn definition_texts<'a>(&'a self, resolution: &DefinitionResolution<'_>) -> Vec<&'a str> {
            let module = parsed_module(&self.db, self.file().python_file(&self.db)).load(&self.db);
            let mut definitions: Vec<_> = resolution
                .definitions()
                .iter()
                .map(|definition| &self.source[definition.full_range(&self.db, &module).range()])
                .collect();
            definitions.sort_unstable();
            definitions
        }

        fn builtin(&self, name: &str) -> DefinitionResolution<'_> {
            let file = self.file();
            let scope = global_scope(&self.db, file);
            let environment = ProgramEnvironment::from_file(file);
            DefinitionResolution::from_builtin(&self.db, &environment, scope, name)
        }

        fn file(&self) -> ProgramFile<'_> {
            let file = system_path_to_file(&self.db, "/src/test.py").expect("test file exists");
            ProgramFile::new(
                &self.db,
                file,
                self.db.program_environment().program(&self.db),
            )
        }
    }
}
