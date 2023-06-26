use std::hash::Hash;

use rspack_core::{
  ChunkGroupOptions, EntryOptions, ModuleDependency, ModuleIdentifier, OutputOptions, SpanExt,
};
use rspack_hash::RspackHash;
use swc_core::common::Spanned;
use swc_core::ecma::{
  ast::{Expr, ExprOrSpread, Ident, Lit, NewExpr},
  atoms::js_word,
  visit::{noop_visit_type, Visit, VisitWith},
};

use super::expr_matcher;
use crate::dependency::WorkerDependency;

// TODO: should created by WorkerPlugin
pub struct WorkerScanner<'a> {
  pub dependencies: &'a mut Vec<Box<dyn ModuleDependency>>,
  index: usize,
  module_identifier: &'a ModuleIdentifier,
  output_options: &'a OutputOptions,
}

// new Worker(new URL("./foo.worker.js", import.meta.url));
impl<'a> WorkerScanner<'a> {
  pub fn new(
    dependencies: &'a mut Vec<Box<dyn ModuleDependency>>,
    module_identifier: &'a ModuleIdentifier,
    output_options: &'a OutputOptions,
  ) -> Self {
    Self {
      dependencies,
      index: 0,
      module_identifier,
      output_options,
    }
  }
}

impl Visit for WorkerScanner<'_> {
  noop_visit_type!();

  fn visit_new_expr(&mut self, new_expr: &NewExpr) {
    if let Some((start, end, request)) = parse_new_worker(new_expr) {
      let mut hasher = RspackHash::with_salt(
        &self.output_options.hash_function,
        &self.output_options.hash_salt,
      );
      self.module_identifier.hash(&mut hasher);
      self.index.hash(&mut hasher);
      self.index += 1;
      let digest = hasher.digest(&self.output_options.hash_digest);
      let runtime = digest
        .rendered(self.output_options.hash_digest_length)
        .to_owned();
      self.dependencies.push(Box::new(WorkerDependency::new(
        start,
        end,
        request,
        Some(new_expr.span.into()),
        ChunkGroupOptions {
          name: None,
          entry_options: Some(EntryOptions {
            runtime: Some(runtime),
            chunk_loading: Some(self.output_options.worker_chunk_loading.clone()),
            async_chunks: None,
            public_path: None,
            base_uri: None,
          }),
        },
      )));
    } else {
      new_expr.visit_children_with(self);
    }
  }
}

fn match_worker_constructor(constructor: &str) -> bool {
  constructor == "Worker" || constructor == "SharedWorker"
}

pub fn parse_new_worker(new_expr: &NewExpr) -> Option<(u32, u32, String)> {
  if let Expr::Ident(Ident { sym, .. }) = &*new_expr.callee
  && match_worker_constructor(sym)
  && let Some(args) = &new_expr.args
  && let Some(expr_or_spread) = args.first()
  && let ExprOrSpread { spread: None, expr: box expr } = expr_or_spread
  && let Expr::New(NewExpr { callee: box callee, args: Some(args), .. }) = expr
  && let Expr::Ident(Ident { sym: js_word!("URL"), .. }) = callee 
  && let (Some(first), Some(second)) = (args.first(), args.get(1))
  && let (
    ExprOrSpread { spread: None, expr: box Expr::Lit(Lit::Str(path)) },
    ExprOrSpread { spread: None, expr: box expr },
  ) = (first, second) && expr_matcher::is_import_meta_url(expr) {
    Some((path.span.real_lo(), expr.span().real_hi(), path.value.to_string()))
  } else {
    None
  }
}
