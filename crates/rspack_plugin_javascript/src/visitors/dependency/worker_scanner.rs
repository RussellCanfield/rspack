use rspack_core::{ChunkGroupOptions, ChunkLoading, EntryOptions, ModuleDependency, SpanExt};
use swc_core::common::Spanned;
use swc_core::ecma::{
  ast::{Expr, ExprOrSpread, Ident, Lit, NewExpr},
  atoms::js_word,
  visit::{noop_visit_type, Visit, VisitWith},
};

use super::expr_matcher;
use crate::dependency::WorkerDependency;

pub struct WorkerScanner<'a> {
  pub dependencies: &'a mut Vec<Box<dyn ModuleDependency>>,
}

// new Worker(new URL("./foo.worker.js", import.meta.url));
impl<'a> WorkerScanner<'a> {
  pub fn new(dependencies: &'a mut Vec<Box<dyn ModuleDependency>>) -> Self {
    Self { dependencies }
  }
}

impl Visit for WorkerScanner<'_> {
  noop_visit_type!();

  fn visit_new_expr(&mut self, new_expr: &NewExpr) {
    if let Expr::Ident(Ident { sym: js_word!("Worker"), .. }) = &*new_expr.callee
    && let Some(args) = &new_expr.args
    && let Some(expr_or_spread) = args.first()
    && let ExprOrSpread { spread: None, expr: box expr } = expr_or_spread
    && let Expr::New(NewExpr { callee: box callee, args: Some(args), .. }) = expr
    && let Expr::Ident(Ident { sym: js_word!("URL"), .. }) = callee 
    && let (Some(first), Some(second)) = (args.first(), args.get(1))
    && let (
      ExprOrSpread {
        spread: None,
        expr: box Expr::Lit(Lit::Str(path)),
      },
      ExprOrSpread {
        spread: None,
        expr: box expr,
      },
    ) = (first, second) && expr_matcher::is_import_meta_url(expr) {
      self.dependencies.push(Box::new(WorkerDependency::new(
        path.span.real_lo(),
        expr.span().real_hi(),
        path.value.to_string(),
        Some(new_expr.span.into()),
        ChunkGroupOptions {
          name: Some("TODO".to_string()),
          entry_options: Some(EntryOptions {
            runtime: Some("TODO".to_string()),
            chunk_loading: Some(ChunkLoading::ImportScripts),
            async_chunks: None,
            public_path: None,
            base_uri: None,
          }),
        }
      )));
    } else {
      new_expr.visit_children_with(self);
    }
  }
}
