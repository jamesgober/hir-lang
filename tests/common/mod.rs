//! Shared helpers for the integration tests.

#![allow(dead_code)]

use hir_lang::{
    Binder, BinderKind, Builder, Effects, ExprId, FnDef, Hir, HirError, Ident, Item, ItemId,
    ItemKind, Name, ParamId, Span, Symbol,
};
use intern_lang::Interner;

/// A builder plus an interner, with short constructors for names.
pub struct Kit {
    pub names: Interner,
    pub b: Builder,
}

impl Kit {
    pub fn new() -> Self {
        Self {
            names: Interner::new(),
            b: Builder::new(),
        }
    }

    pub fn sym(&mut self, s: &str) -> Symbol {
        self.names.intern(s)
    }

    pub fn name(&mut self, s: &str) -> Name {
        Name::new(self.sym(s))
    }

    pub fn ident(&mut self, s: &str) -> Ident {
        Ident::new(self.sym(s), Span::empty(0))
    }

    pub fn binder(&mut self, s: &str, kind: BinderKind) -> hir_lang::BinderId {
        let name = self.name(s);
        self.b.binder(Binder::new(name, kind))
    }

    /// `fn name(params) <effects> { body }`.
    pub fn func_fx(
        &mut self,
        name: &str,
        params: &[ParamId],
        effects: Effects,
        body: ExprId,
    ) -> ItemId {
        let name = self.name(name);
        let params = self.b.list(params);
        self.b.item(Item::new(
            Some(name),
            ItemKind::Fn(FnDef {
                params,
                effects,
                body: Some(body),
                ..FnDef::default()
            }),
        ))
    }

    /// Wraps `body` as the body of `fn main()` in a root module and finishes.
    pub fn finish_body(self, body: ExprId) -> Result<Hir, HirError> {
        self.finish_body_fx(body, Effects::NONE)
    }

    pub fn finish_body_fx(mut self, body: ExprId, effects: Effects) -> Result<Hir, HirError> {
        let f = self.func_fx("main", &[], effects, body);
        let root = self.b.module(None, &[f]);
        self.b.finish(root)
    }

    /// Like `finish_body`, keeping the interner for printing.
    pub fn finish_body_keep(mut self, body: ExprId) -> (Result<Hir, HirError>, Interner) {
        let f = self.func_fx("main", &[], Effects::NONE, body);
        let root = self.b.module(None, &[f]);
        (self.b.finish(root), self.names)
    }

    /// Like `finish_items`, keeping the interner for printing.
    pub fn finish_items_keep(mut self, items: &[ItemId]) -> (Result<Hir, HirError>, Interner) {
        let root = self.b.module(None, items);
        (self.b.finish(root), self.names)
    }

    /// Finishes with the given items in a root module.
    pub fn finish_items(mut self, items: &[ItemId]) -> Result<Hir, HirError> {
        let root = self.b.module(None, items);
        self.b.finish(root)
    }
}
