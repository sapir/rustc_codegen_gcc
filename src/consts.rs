use std::borrow::Cow;
use std::ffi::CStr;

use gccjit::{Context, GlobalKind, LValue, RValue, ToRValue, Type};
use rustc_codegen_ssa::traits::{BaseTypeMethods, BuilderMethods, ConstMethods, DeclareMethods, DerivedTypeMethods, StaticMethods};
use rustc_hir as hir;
use rustc_hir::Node;
use rustc_middle::{bug, span_bug};
use rustc_middle::middle::codegen_fn_attrs::{CodegenFnAttrFlags, CodegenFnAttrs};
use rustc_middle::mir::mono::MonoItem;
use rustc_middle::ty::{self, Instance, Ty};
use rustc_mir::interpret::{self, Allocation, ConstValue, ErrorHandled, read_target_uint};
use rustc_span::Span;
use rustc_span::def_id::DefId;
use rustc_span::symbol::{sym, Symbol};
use rustc_target::abi::{self, Align, HasDataLayout, LayoutOf, Primitive, Size};

use crate::base;
use crate::common::type_is_pointer;
use crate::context::CodegenCx;
use crate::type_of::LayoutGccExt;

impl<'gcc, 'tcx> CodegenCx<'gcc, 'tcx> {
    pub fn const_bitcast(&self, value: RValue<'gcc>, typ: Type<'gcc>) -> RValue<'gcc> {
        self.context.new_cast(None, value, typ)
        // FIXME: use a real bitcast.
            /*
        //println!("bitcast: {:?} -> {:?}", value, dest_ty);
        if type_is_pointer(value.get_type()) && type_is_pointer(typ) {
            return self.context.new_cast(None, value, typ);
        }

        let func = self.current_func.borrow().expect("current func");
        let variable = func.new_local(None, value.get_type(), "pointerCastVar");
        // FIXME: we might not be in a function here, so we cannot create a variable.
        // Use a global? Where to init it, though? Maybe where this function is called (static
        // creation).
        self.current_block.borrow().expect("current block").add_assignment(None, variable, value);
        let pointer = variable.get_address(None);
        self.context.new_cast(None, pointer, typ.make_pointer())
            .dereference(None)
            .to_rvalue()
            */
    }
}

impl<'gcc, 'tcx> StaticMethods for CodegenCx<'gcc, 'tcx> {
    fn static_addr_of(&self, cv: RValue<'gcc>, align: Align, kind: Option<&str>) -> RValue<'gcc> {
        if let Some(global_value) = self.const_globals.borrow().get(&cv) {
            // TODO
            /*unsafe {
                // Upgrade the alignment in cases where the same constant is used with different
                // alignment requirements
                let llalign = align.bytes() as u32;
                if llalign > llvm::LLVMGetAlignment(gv) {
                    llvm::LLVMSetAlignment(gv, llalign);
                }
            }*/
            return *global_value;
        }
        let global_value = self.static_addr_of_mut(cv, align, kind);
        // TODO
        /*unsafe {
            llvm::LLVMSetGlobalConstant(global_value, True);
        }*/
        self.const_globals.borrow_mut().insert(cv, global_value);
        global_value
    }

    fn codegen_static(&self, def_id: DefId, is_mutable: bool) {
        unsafe {
            let attrs = self.tcx.codegen_fn_attrs(def_id);

            let (value, alloc) =
                match codegen_static_initializer(&self, def_id) {
                    Ok(value) => value,
                    // Error has already been reported
                    Err(_) => return,
                };

            let global = self.get_static(def_id);

            // boolean SSA values are i1, but they have to be stored in i8 slots,
            // otherwise some LLVM optimization passes don't work as expected
            let mut val_llty = self.val_ty(value);
            let value =
                if val_llty == self.type_i1() {
                    val_llty = self.type_i8();
                    unimplemented!();
                    //llvm::LLVMConstZExt(value, val_llty)
                }
                else {
                    value
                };

            let instance = Instance::mono(self.tcx, def_id);
            let ty = instance.monomorphic_ty(self.tcx);
            let gcc_type = self.layout_of(ty).gcc_type(self, true);
            let global =
                if val_llty == gcc_type {
                    global
                }
                else {
                    // If we created the global with the wrong type,
                    // correct the type.
                    let name = self.get_global_name(global).expect("global name");
                    /*let name = llvm::get_value_name(global).to_vec();
                    llvm::set_value_name(global, b"");

                    let linkage = llvm::LLVMRustGetLinkage(global);
                    let visibility = llvm::LLVMRustGetVisibility(global);*/

                    let new_global = self.get_or_insert_global(&name, val_llty);

                    /*llvm::LLVMRustSetLinkage(new_global, linkage);
                      llvm::LLVMRustSetVisibility(new_global, visibility);*/

                    // To avoid breaking any invariants, we leave around the old
                    // global for the moment; we'll replace all references to it
                    // with the new global later. (See base::codegen_backend.)
                    //self.statics_to_rauw.borrow_mut().push((global, new_global));
                    new_global
                };
            // TODO
            //set_global_alignment(&self, global, self.align_of(ty));
            //llvm::LLVMSetInitializer(global, value);
            let value = self.rvalue_as_lvalue(value);
            let value = value.get_address(None);
            let lvalue = global.dereference(None);
            let dest_typ = global.get_type();
            let value = self.context.new_cast(None, value, dest_typ);

            // TODO: switch to set_initializer when libgccjit supports that.
            let memcpy = self.context.get_builtin_function("memcpy");
            let dst = self.context.new_cast(None, global, self.type_i8p());
            let src = self.context.new_cast(None, value, self.type_ptr_to(self.type_void()));
            let size = self.context.new_rvalue_from_long(self.sizet_type, alloc.size.bytes() as i64);
            self.global_init_block.add_eval(None, self.context.new_call(None, memcpy, &[dst, src, size]));

            // As an optimization, all shared statics which do not have interior
            // mutability are placed into read-only memory.
            if !is_mutable {
                if self.type_is_freeze(ty) {
                    // TODO
                    //llvm::LLVMSetGlobalConstant(global, llvm::True);
                }
            }

            //debuginfo::create_global_var_metadata(&self, def_id, global);

            if attrs.flags.contains(CodegenFnAttrFlags::THREAD_LOCAL) {
                // TODO
                //llvm::set_thread_local_mode(global, self.tls_model);

                // Do not allow LLVM to change the alignment of a TLS on macOS.
                //
                // By default a global's alignment can be freely increased.
                // This allows LLVM to generate more performant instructions
                // e.g., using load-aligned into a SIMD register.
                //
                // However, on macOS 10.10 or below, the dynamic linker does not
                // respect any alignment given on the TLS (radar 24221680).
                // This will violate the alignment assumption, and causing segfault at runtime.
                //
                // This bug is very easy to trigger. In `println!` and `panic!`,
                // the `LOCAL_STDOUT`/`LOCAL_STDERR` handles are stored in a TLS,
                // which the values would be `mem::replace`d on initialization.
                // The implementation of `mem::replace` will use SIMD
                // whenever the size is 32 bytes or higher. LLVM notices SIMD is used
                // and tries to align `LOCAL_STDOUT`/`LOCAL_STDERR` to a 32-byte boundary,
                // which macOS's dyld disregarded and causing crashes
                // (see issues #51794, #51758, #50867, #48866 and #44056).
                //
                // To workaround the bug, we trick LLVM into not increasing
                // the global's alignment by explicitly assigning a section to it
                // (equivalent to automatically generating a `#[link_section]` attribute).
                // See the comment in the `GlobalValue::canIncreaseAlignment()` function
                // of `lib/IR/Globals.cpp` for why this works.
                //
                // When the alignment is not increased, the optimized `mem::replace`
                // will use load-unaligned instructions instead, and thus avoiding the crash.
                //
                // We could remove this hack whenever we decide to drop macOS 10.10 support.
                if self.tcx.sess.target.target.options.is_like_osx {
                    // The `inspect` method is okay here because we checked relocations, and
                    // because we are doing this access to inspect the final interpreter state
                    // (not as part of the interpreter execution).
                    //
                    // FIXME: This check requires that the (arbitrary) value of undefined bytes
                    // happens to be zero. Instead, we should only check the value of defined bytes
                    // and set all undefined bytes to zero if this allocation is headed for the
                    // BSS.
                    let all_bytes_are_zero = alloc.relocations().is_empty()
                        && alloc
                            .inspect_with_undef_and_ptr_outside_interpreter(0..alloc.len())
                            .iter()
                            .all(|&byte| byte == 0);

                    let sect_name = if all_bytes_are_zero {
                        CStr::from_bytes_with_nul_unchecked(b"__DATA,__thread_bss\0")
                    } else {
                        CStr::from_bytes_with_nul_unchecked(b"__DATA,__thread_data\0")
                    };
                    unimplemented!();
                    //llvm::LLVMSetSection(global, sect_name.as_ptr());
                }
            }

            // Wasm statics with custom link sections get special treatment as they
            // go into custom sections of the wasm executable.
            if self.tcx.sess.opts.target_triple.triple().starts_with("wasm32") {
                if let Some(section) = attrs.link_section {
                    unimplemented!();
                    /*let section = llvm::LLVMMDStringInContext(
                        self.llcx,
                        section.as_str().as_ptr().cast(),
                        section.as_str().len() as c_uint,
                    );
                    assert!(alloc.relocations().is_empty());

                    // The `inspect` method is okay here because we checked relocations, and
                    // because we are doing this access to inspect the final interpreter state (not
                    // as part of the interpreter execution).
                    let bytes =
                        alloc.inspect_with_undef_and_ptr_outside_interpreter(0..alloc.len());
                    let alloc = llvm::LLVMMDStringInContext(
                        self.llcx,
                        bytes.as_ptr().cast(),
                        bytes.len() as c_uint,
                    );
                    let data = [section, alloc];
                    let meta = llvm::LLVMMDNodeInContext(self.llcx, data.as_ptr(), 2);
                    llvm::LLVMAddNamedMetadataOperand(
                        self.llmod,
                        "wasm.custom_sections\0".as_ptr().cast(),
                        meta,
                    );*/
                }
            } else {
                // TODO
                //base::set_link_section(global, &attrs);
            }

            if attrs.flags.contains(CodegenFnAttrFlags::USED) {
                // This static will be stored in the llvm.used variable which is an array of i8*
                let cast = self.context.new_cast(None, global, self.type_i8p());
                //self.used_statics.borrow_mut().push(cast);
            }
        }
    }
}

impl<'gcc, 'tcx> CodegenCx<'gcc, 'tcx> {
    pub fn static_addr_of_mut(&self, cv: RValue<'gcc>, align: Align, kind: Option<&str>) -> RValue<'gcc> {
        let (name, gv) =
            match kind {
                Some(kind) if !self.tcx.sess.fewer_names() => {
                    let name = self.generate_local_symbol_name(kind);
                    let gv = self.define_global(&name[..], self.val_ty(cv)).unwrap_or_else(|| {
                        bug!("symbol `{}` is already defined", name);
                    });
                    //llvm::LLVMRustSetLinkage(gv, llvm::Linkage::PrivateLinkage);
                    (name, gv)
                }
                _ => {
                    let index = self.global_gen_sym_counter.get();
                    let name = format!("global_{}_{}", index, self.codegen_unit.name());
                    let global = self.define_private_global(self.val_ty(cv));
                    (name, global)
                },
            };
        // FIXME: I think the name coming from generate_local_symbol_name() above cannot be used
        // globally.
        // NOTE: global seems to only be global in a module. So save the name instead of the value
        // to import it later.
        self.global_names.borrow_mut().insert(cv, name);
        self.global_init_block.add_assignment(None, gv.dereference(None), cv);
        //set_global_alignment(&self, gv, align);
        //llvm::SetUnnamedAddress(gv, llvm::UnnamedAddr::Global);
        gv
    }

    pub fn get_static(&self, def_id: DefId) -> RValue<'gcc> {
        let instance = Instance::mono(self.tcx, def_id);
        if let Some(&global) = self.instances.borrow().get(&instance) {
            let attrs = self.tcx.codegen_fn_attrs(def_id);
            let name = &*self.tcx.symbol_name(instance).name.as_str();
            let name =
                if let Some(linkage) = attrs.linkage {
                    // This is to match what happens in check_and_apply_linkage.
                    Cow::from(format!("_rust_extern_with_linkage_{}", name))
                }
                else {
                    Cow::from(name)
                };
            let global = self.context.new_global(None, GlobalKind::Imported, global.get_type(), &name)
                .get_address(None);
            self.global_names.borrow_mut().insert(global, name.to_string());
            return global;
        }

        let defined_in_current_codegen_unit =
            self.codegen_unit.items().contains_key(&MonoItem::Static(def_id));
        assert!(
            !defined_in_current_codegen_unit,
            "consts::get_static() should always hit the cache for \
                 statics defined in the same CGU, but did not for `{:?}`",
            def_id
        );

        let ty = instance.monomorphic_ty(self.tcx);
        let sym = self.tcx.symbol_name(instance).name;

        //debug!("get_static: sym={} instance={:?}", sym, instance);

        let global =
            if let Some(def_id) = def_id.as_local() {
                let id = self.tcx.hir().as_local_hir_id(def_id);
                let llty = self.layout_of(ty).gcc_type(self, true);
                // FIXME: refactor this to work without accessing the HIR
                let (global, attrs) = match self.tcx.hir().get(id) {
                    Node::Item(&hir::Item { attrs, span, kind: hir::ItemKind::Static(..), .. }) => {
                        let sym_str = sym.as_str();
                        if let Some(global) = self.get_declared_value(&sym_str) {
                            if self.val_ty(global) != self.type_ptr_to(llty) {
                                span_bug!(span, "Conflicting types for static");
                            }
                        }

                        let global = self.declare_global(&sym_str, llty);

                        if !self.tcx.is_reachable_non_generic(def_id) {
                            /*unsafe {
                              llvm::LLVMRustSetVisibility(global, llvm::Visibility::Hidden);
                              }*/
                        }

                        (global, attrs)
                    }

                    Node::ForeignItem(&hir::ForeignItem {
                        ref attrs,
                        span,
                        kind: hir::ForeignItemKind::Static(..),
                        ..
                    }) => {
                        let fn_attrs = self.tcx.codegen_fn_attrs(def_id);
                        (check_and_apply_linkage(&self, &fn_attrs, ty, sym, span), &**attrs)
                    }

                    item => bug!("get_static: expected static, found {:?}", item),
                };

                //debug!("get_static: sym={} attrs={:?}", sym, attrs);

                for attr in attrs {
                    if attr.check_name(sym::thread_local) {
                        //llvm::set_thread_local_mode(global, self.tls_model);
                    }
                }

                global
            }
            else {
                // FIXME(nagisa): perhaps the map of externs could be offloaded to llvm somehow?
                //debug!("get_static: sym={} item_attr={:?}", sym, self.tcx.item_attrs(def_id));

                let attrs = self.tcx.codegen_fn_attrs(def_id);
                let span = self.tcx.def_span(def_id);
                let global = check_and_apply_linkage(&self, &attrs, ty, sym, span);

                // Thread-local statics in some other crate need to *always* be linked
                // against in a thread-local fashion, so we need to be sure to apply the
                // thread-local attribute locally if it was present remotely. If we
                // don't do this then linker errors can be generated where the linker
                // complains that one object files has a thread local version of the
                // symbol and another one doesn't.
                if attrs.flags.contains(CodegenFnAttrFlags::THREAD_LOCAL) {
                    unimplemented!();
                    //llvm::set_thread_local_mode(global, self.tls_model);
                }

                let needs_dll_storage_attr = false; /*self.use_dll_storage_attrs && !self.tcx.is_foreign_item(def_id) &&
                // ThinLTO can't handle this workaround in all cases, so we don't
                // emit the attrs. Instead we make them unnecessary by disallowing
                // dynamic linking when linker plugin based LTO is enabled.
                !self.tcx.sess.opts.cg.linker_plugin_lto.enabled();*/

                // If this assertion triggers, there's something wrong with commandline
                // argument validation.
                debug_assert!(
                    !(self.tcx.sess.opts.cg.linker_plugin_lto.enabled()
                        && self.tcx.sess.target.target.options.is_like_msvc
                        && self.tcx.sess.opts.cg.prefer_dynamic)
                );

                if needs_dll_storage_attr {
                    // This item is external but not foreign, i.e., it originates from an external Rust
                    // crate. Since we don't know whether this crate will be linked dynamically or
                    // statically in the final application, we always mark such symbols as 'dllimport'.
                    // If final linkage happens to be static, we rely on compiler-emitted __imp_ stubs
                    // to make things work.
                    //
                    // However, in some scenarios we defer emission of statics to downstream
                    // crates, so there are cases where a static with an upstream DefId
                    // is actually present in the current crate. We can find out via the
                    // is_codegened_item query.
                    if !self.tcx.is_codegened_item(def_id) {
                        unimplemented!();
                        unsafe {
                            //llvm::LLVMSetDLLStorageClass(global, llvm::DLLStorageClass::DllImport);
                        }
                    }
                }
                global
            };

        /*if self.use_dll_storage_attrs && self.tcx.is_dllimport_foreign_item(def_id) {
            // For foreign (native) libs we know the exact storage type to use.
            unsafe {
                llvm::LLVMSetDLLStorageClass(global, llvm::DLLStorageClass::DllImport);
            }
        }*/

        self.instances.borrow_mut().insert(instance, global);
        global
    }
}

pub fn const_alloc_to_gcc<'gcc, 'tcx>(cx: &CodegenCx<'gcc, 'tcx>, alloc: &Allocation) -> RValue<'gcc> {
    let mut llvals = Vec::with_capacity(alloc.relocations().len() + 1);
    let dl = cx.data_layout();
    let pointer_size = dl.pointer_size.bytes() as usize;

    let mut next_offset = 0;
    for &(offset, ((), alloc_id)) in alloc.relocations().iter() {
        let offset = offset.bytes();
        assert_eq!(offset as usize as u64, offset);
        let offset = offset as usize;
        if offset > next_offset {
            // This `inspect` is okay since we have checked that it is not within a relocation, it
            // is within the bounds of the allocation, and it doesn't affect interpreter execution
            // (we inspect the result after interpreter execution). Any undef byte is replaced with
            // some arbitrary byte value.
            //
            // FIXME: relay undef bytes to codegen as undef const bytes
            let bytes = alloc.inspect_with_undef_and_ptr_outside_interpreter(next_offset..offset);
            llvals.push(cx.const_bytes(bytes));
        }
        let ptr_offset =
            read_target_uint( dl.endian,
                // This `inspect` is okay since it is within the bounds of the allocation, it doesn't
                // affect interpreter execution (we inspect the result after interpreter execution),
                // and we properly interpret the relocation as a relocation pointer offset.
                alloc.inspect_with_undef_and_ptr_outside_interpreter(offset..(offset + pointer_size)),
            )
            .expect("const_alloc_to_llvm: could not read relocation pointer")
            as u64;
        llvals.push(cx.scalar_to_backend(
            interpret::Pointer::new(alloc_id, Size::from_bytes(ptr_offset)).into(),
            &abi::Scalar { value: Primitive::Pointer, valid_range: 0..=!0 },
            cx.type_i8p(),
        ));
        next_offset = offset + pointer_size;
    }
    if alloc.len() >= next_offset {
        let range = next_offset..alloc.len();
        // This `inspect` is okay since we have check that it is after all relocations, it is
        // within the bounds of the allocation, and it doesn't affect interpreter execution (we
        // inspect the result after interpreter execution). Any undef byte is replaced with some
        // arbitrary byte value.
        //
        // FIXME: relay undef bytes to codegen as undef const bytes
        let bytes = alloc.inspect_with_undef_and_ptr_outside_interpreter(range);
        llvals.push(cx.const_bytes(bytes));
    }

    cx.const_struct(&llvals, true)
}

pub fn codegen_static_initializer<'gcc, 'tcx>(cx: &CodegenCx<'gcc, 'tcx>, def_id: DefId) -> Result<(RValue<'gcc>, &'tcx Allocation), ErrorHandled> {
    let alloc =
        match cx.tcx.const_eval_poly(def_id)? {
            ConstValue::ByRef { alloc, offset } if offset.bytes() == 0 => alloc,
            val => bug!("static const eval returned {:#?}", val),
        };
    Ok((const_alloc_to_gcc(cx, alloc), alloc))
}

fn check_and_apply_linkage<'gcc, 'tcx>(cx: &CodegenCx<'gcc, 'tcx>, attrs: &CodegenFnAttrs, ty: Ty<'tcx>, sym: Symbol, span: Span) -> RValue<'gcc> {
    let llty = cx.layout_of(ty).gcc_type(cx, true);
    let sym = sym.as_str();
    if let Some(linkage) = attrs.linkage {
        //debug!("get_static: sym={} linkage={:?}", sym, linkage);

        // If this is a static with a linkage specified, then we need to handle
        // it a little specially. The typesystem prevents things like &T and
        // extern "C" fn() from being non-null, so we can't just declare a
        // static and call it a day. Some linkages (like weak) will make it such
        // that the static actually has a null value.
        let llty2 =
            if let ty::RawPtr(ref mt) = ty.kind {
                cx.layout_of(mt.ty).gcc_type(cx, true)
            }
            else {
                cx.sess().span_fatal(
                    span,
                    "must have type `*const T` or `*mut T` due to `#[linkage]` attribute",
                )
            };
        unsafe {
            // Declare a symbol `foo` with the desired linkage.
            let global1 = cx.declare_global_with_linkage(&sym, llty2, base::global_linkage_to_gcc(linkage));

            // Declare an internal global `extern_with_linkage_foo` which
            // is initialized with the address of `foo`.  If `foo` is
            // discarded during linking (for example, if `foo` has weak
            // linkage and there are no definitions), then
            // `extern_with_linkage_foo` will instead be initialized to
            // zero.
            let mut real_name = "_rust_extern_with_linkage_".to_string();
            real_name.push_str(&sym);
            let global2 =
                cx.define_global(&real_name, llty).unwrap_or_else(|| {
                    cx.sess().span_fatal(span, &format!("symbol `{}` is already defined", &sym))
                });
            //llvm::LLVMRustSetLinkage(global2, llvm::Linkage::InternalLinkage);
            let lvalue = global2.dereference(None);
            cx.global_init_block.add_assignment(None, lvalue, global1);
            //llvm::LLVMSetInitializer(global2, global1);
            global2
        }
    }
    else {
        // Generate an external declaration.
        // FIXME(nagisa): investigate whether it can be changed into define_global
        cx.declare_global(&sym, llty)
    }
}
