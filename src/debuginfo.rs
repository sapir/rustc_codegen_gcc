use gccjit::{Location, RValue};
use rustc_codegen_ssa::mir::debuginfo::{DebugScope, FunctionDebugContext, VariableKind};
use rustc_codegen_ssa::traits::{DebugInfoBuilderMethods, DebugInfoMethods};
use rustc_index::bit_set::BitSet;
use rustc_index::IndexVec;
use rustc_middle::mir::{Body, self, SourceScope};
use rustc_middle::ty::{Instance, PolyExistentialTraitRef, Ty};
use rustc_session::config::DebugInfo;
use rustc_span::{BytePos, Pos, SourceFile, SourceFileAndLine, Span, Symbol};
use rustc_target::abi::call::FnAbi;
use rustc_target::abi::Size;
use rustc_data_structures::sync::Lrc;
use crate::rustc_index::Idx;
use std::ops::Range;

use crate::builder::Builder;
use crate::context::CodegenCx;

pub(super) const UNKNOWN_LINE_NUMBER: u32 = 0;
pub(super) const UNKNOWN_COLUMN_NUMBER: u32 = 0;

impl<'a, 'gcc, 'tcx> DebugInfoBuilderMethods for Builder<'a, 'gcc, 'tcx> {
    // FIXME(eddyb) find a common convention for all of the debuginfo-related
    // names (choose between `dbg`, `debug`, `debuginfo`, `debug_info` etc.).
    fn dbg_var_addr(
        &mut self,
        _dbg_var: Self::DIVariable,
        dbg_loc: Self::DILocation,
        variable_alloca: Self::Value,
        _direct_offset: Size,
        _indirect_offsets: &[Size],
        _fragment: Option<Range<Size>>,
    ) {
        // FIXME(tempdragon): Not sure if this is correct, probably wrong but still keep it here.
        #[cfg(feature = "master")]
        variable_alloca.set_location(dbg_loc);
    }

    fn insert_reference_to_gdb_debug_scripts_section_global(&mut self) {
        // TODO(antoyo): insert reference to gdb debug scripts section global.
    }

    /// FIXME(tempdragon): Currently, this function is not yet implemented. It seems that the
    /// debug name and the mangled name should both be included in the LValues.
    /// Besides, a function to get the rvalue type(m_is_lvalue) should also be included.
    fn set_var_name(&mut self, _value: RValue<'gcc>, _name: &str) {
    }

    fn set_dbg_loc(&mut self, dbg_loc: Self::DILocation) {
        self.loc = Some(dbg_loc);
    }
}

pub fn compute_mir_scopes<'gcc, 'tcx>(
    cx: &CodegenCx<'gcc, 'tcx>,
    instance: Instance<'tcx>,
    mir: &Body<'tcx>,
    debug_context: &mut FunctionDebugContext<'tcx, (), Location<'gcc>>,
) {
    // Find all scopes with variables defined in them.
    let variables = if cx.sess().opts.debuginfo == DebugInfo::Full {
        let mut vars = BitSet::new_empty(mir.source_scopes.len());
        // FIXME(eddyb) take into account that arguments always have debuginfo,
        // irrespective of their name (assuming full debuginfo is enabled).
        // NOTE(eddyb) actually, on second thought, those are always in the
        // function scope, which always exists.
        for var_debug_info in &mir.var_debug_info {
            vars.insert(var_debug_info.source_info.scope);
        }
        Some(vars)
    } else {
        // Nothing to emit, of course.
        None
    };
    let mut instantiated = BitSet::new_empty(mir.source_scopes.len());
    // Instantiate all scopes.
    for idx in 0..mir.source_scopes.len() {
        let scope = SourceScope::new(idx);
        make_mir_scope(cx, instance, mir, &variables, debug_context, &mut instantiated, scope);
    }
    assert!(instantiated.count() == mir.source_scopes.len());
}

fn make_mir_scope<'gcc, 'tcx>(
    cx: &CodegenCx<'gcc, 'tcx>,
    instance: Instance<'tcx>,
    mir: &Body<'tcx>,
    variables: &Option<BitSet<SourceScope>>,
    debug_context: &mut FunctionDebugContext<'tcx, (), Location<'gcc>>,
    instantiated: &mut BitSet<SourceScope>,
    scope: SourceScope,
) {
    if instantiated.contains(scope) {
        return;
    }

    let scope_data = &mir.source_scopes[scope];
    let parent_scope = if let Some(parent) = scope_data.parent_scope {
        make_mir_scope(cx, instance, mir, variables, debug_context, instantiated, parent);
        debug_context.scopes[parent]
    } else {
        // The root is the function itself.
        let file = cx.sess().source_map().lookup_source_file(mir.span.lo());
        debug_context.scopes[scope] = DebugScope {
            file_start_pos: file.start_pos,
            file_end_pos: file.end_position(),
            ..debug_context.scopes[scope]
        };
        instantiated.insert(scope);
        return;
    };

    if let Some(vars) = variables
    {
        if !vars.contains(scope)
            && scope_data.inlined.is_none() {
                // Do not create a DIScope if there are no variables defined in this
                // MIR `SourceScope`, and it's not `inlined`, to avoid debuginfo bloat.
                debug_context.scopes[scope] = parent_scope;
                instantiated.insert(scope);
                return;
            }
    }

    let loc = cx.lookup_debug_loc(scope_data.span.lo());
    let dbg_scope = ();

    let inlined_at = scope_data.inlined.map(|(_, callsite_span)| {
        // FIXME(eddyb) this doesn't account for the macro-related
        // `Span` fixups that `rustc_codegen_ssa::mir::debuginfo` does.
        let callsite_scope = parent_scope.adjust_dbg_scope_for_span(cx, callsite_span);
        cx.dbg_loc(callsite_scope, parent_scope.inlined_at, callsite_span)
    });
    let p_inlined_at = parent_scope.inlined_at;
    // TODO(tempdragon): dbg_scope: Add support for scope extension here.
    inlined_at.or(p_inlined_at);
    
    debug_context.scopes[scope] = DebugScope {
        dbg_scope,
        inlined_at,
        file_start_pos: loc.0.start_pos,
        file_end_pos: loc.0.end_position(),
    };
    instantiated.insert(scope);
}

/// Copied from LLVM backend
impl<'gcc, 'tcx> CodegenCx<'gcc, 'tcx> {
    pub fn lookup_debug_loc(&self, pos: BytePos) -> (Lrc<SourceFile>, u32, u32) {
        match self.sess().source_map().lookup_line(pos) {
            Ok(SourceFileAndLine { sf: file, line }) => {
                let line_pos = file.lines()[line];

                // Use 1-based indexing.
                let line = (line + 1) as u32;
                let col = (file.relative_position(pos) - line_pos).to_u32() + 1;
                (file,
                 line,
                 if ! self.sess().target.is_like_msvc {
                     col } else {
                     UNKNOWN_COLUMN_NUMBER
                 }
                )
            }
            Err(file) => (file, UNKNOWN_LINE_NUMBER, UNKNOWN_COLUMN_NUMBER),
        }
    }
}

impl<'gcc, 'tcx> DebugInfoMethods<'tcx> for CodegenCx<'gcc, 'tcx> {
    fn create_vtable_debuginfo(
        &self,
        _ty: Ty<'tcx>,
        _trait_ref: Option<PolyExistentialTraitRef<'tcx>>,
        _vtable: Self::Value,
    ) {
        // TODO(antoyo)
    }

    fn create_function_debug_context(
        &self,
        instance: Instance<'tcx>,
        fn_abi: &FnAbi<'tcx, Ty<'tcx>>,
        llfn: RValue<'gcc>,
        mir: &mir::Body<'tcx>,
    ) -> Option<FunctionDebugContext<'tcx, Self::DIScope, Self::DILocation>> {
        // TODO(antoyo)
        if self.sess().opts.debuginfo == DebugInfo::None {
            return None;
        }

        // Initialize fn debug context (including scopes).
        let empty_scope = DebugScope {
            dbg_scope: self.dbg_scope_fn(instance, fn_abi, Some(llfn)),
            inlined_at: None,
            file_start_pos: BytePos(0),
            file_end_pos: BytePos(0),
        };
        let mut fn_debug_context = FunctionDebugContext {
            scopes: IndexVec::from_elem(empty_scope, &mir.source_scopes.as_slice()),
            inlined_function_scopes: Default::default(),
        };

        // Fill in all the scopes, with the information from the MIR body.
        compute_mir_scopes(self, instance, mir, &mut fn_debug_context);

        Some(fn_debug_context)
    }

    fn extend_scope_to_file(
        &self,
        _scope_metadata: Self::DIScope,
        _file: &SourceFile,
    ) -> Self::DIScope {
        // TODO(antoyo): implement.
    }

    fn debuginfo_finalize(&self) {
        // TODO(antoyo): Get the debug flag/predicate to allow optional generation of debuginfo.
        self.context.set_debug_info(true)
    }

    fn create_dbg_var(
        &self,
        _variable_name: Symbol,
        _variable_type: Ty<'tcx>,
        _scope_metadata: Self::DIScope,
        _variable_kind: VariableKind,
        _span: Span,
    ) -> Self::DIVariable {
        ()
    }

    fn dbg_scope_fn(
        &self,
        _instance: Instance<'tcx>,
        _fn_abi: &FnAbi<'tcx, Ty<'tcx>>,
        _maybe_definition_llfn: Option<RValue<'gcc>>,
    ) -> Self::DIScope {
        // TODO(antoyo): implement.
    }

    fn dbg_loc(
        &self,
        _scope: Self::DIScope,
        _inlined_at: Option<Self::DILocation>,
        span: Span,
    ) -> Self::DILocation {
        let pos = span.lo();
        let (file, line, col) = self.lookup_debug_loc(pos);
        let loc = match &file.name {
            rustc_span::FileName::Real(name) => match name {
                rustc_span::RealFileName::LocalPath(name) => {
                    if let Some(name) = name.to_str() {
                        self.context
                            .new_location(name, line as i32, col as i32)
                    } else{
                        Location::null()
                    }
                }
                rustc_span::RealFileName::Remapped {
                    local_path,
                    virtual_name:_,
                } => if let Some(name) = local_path.as_ref() {
                    if let Some(name) = name.to_str(){
                        self.context.new_location(
                            name,
                            line as i32,
                            col as i32,
                        )
                    } else {
                        Location::null()
                    }
                } else{
                    Location::null()
                },
            },
            _ => Location::null(),
        };
        loc
    }
}
