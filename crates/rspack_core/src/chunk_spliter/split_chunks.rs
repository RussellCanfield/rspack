use anyhow::anyhow;
use std::collections::{HashMap, HashSet};

// use crate::{
//     BundleOptions, Chunk, ChunkGraph, ChunkIdAlgo, ChunkKind, JsModuleKind, ModuleGraphContainer,
// };

use rspack_error::Result;
use tracing::instrument;

use crate::{
  uri_to_chunk_name, ChunkGroup, ChunkGroupKind, ChunkGroupUkey, ChunkUkey, Compilation,
  ModuleIdentifier,
};

#[instrument(skip_all)]
pub fn code_splitting(compilation: &mut Compilation) -> Result<()> {
  CodeSplitter::new(compilation).split()?;
  Ok(())
}

struct CodeSplitter<'me> {
  compilation: &'me mut Compilation,
  next_free_module_pre_order_index: usize,
  next_free_module_post_order_index: usize,
  queue: Vec<QueueItem>,
  queue_delayed: Vec<QueueItem>,
  chunk_relation_graph: petgraph::graphmap::DiGraphMap<ChunkUkey, ()>,
  split_point_modules: HashSet<ModuleIdentifier>,
}

impl<'me> CodeSplitter<'me> {
  pub fn new(compilation: &'me mut Compilation) -> Self {
    CodeSplitter {
      compilation,
      next_free_module_pre_order_index: 0,
      next_free_module_post_order_index: 0,
      queue: Default::default(),
      queue_delayed: Default::default(),
      chunk_relation_graph: Default::default(),
      split_point_modules: Default::default(),
    }
  }

  fn prepare_input_entrypoints_and_modules(
    &mut self,
  ) -> Result<HashMap<ChunkGroupUkey, Vec<ModuleIdentifier>>> {
    let compilation = &mut self.compilation;
    let module_graph = &compilation.module_graph;

    let entries = compilation.entry_data();

    let mut input_entrypoints_and_modules: HashMap<ChunkGroupUkey, Vec<ModuleIdentifier>> =
      HashMap::new();

    for (name, entry_data) in entries.iter() {
      let options = &entry_data.options;
      let dependencies = &entry_data.dependencies;
      let module_identifiers = dependencies
        .iter()
        .filter_map(|dep| {
          module_graph
            .module_by_dependency(dep)
            .map(|module| module.module_identifier)
        })
        .collect::<Vec<_>>();
      let chunk = Compilation::add_named_chunk(
        name.to_string(),
        name.to_string(),
        &mut compilation.chunk_by_ukey,
        &mut compilation.named_chunks,
      );

      compilation.chunk_graph.add_chunk(chunk.ukey);

      for module_identifier in module_identifiers {
        compilation
          .chunk_graph
          .split_point_module_identifier_to_chunk_ukey
          .insert(module_identifier, chunk.ukey);
      }

      let mut entrypoint = ChunkGroup::new(ChunkGroupKind::Entrypoint, Some(name.to_string()));
      if options.runtime.is_none() {
        entrypoint.set_runtime_chunk(chunk.ukey);
      }
      entrypoint.set_entry_point_chunk(chunk.ukey);
      entrypoint.connect_chunk(chunk);

      compilation
        .named_chunk_groups
        .insert(name.to_string(), entrypoint.ukey);

      compilation
        .entrypoints
        .insert(name.to_string(), entrypoint.ukey);

      let entrypoint = {
        let ukey = entrypoint.ukey;
        compilation.chunk_group_by_ukey.insert(ukey, entrypoint);

        compilation
          .chunk_group_by_ukey
          .get(&ukey)
          .ok_or_else(|| anyhow::format_err!("no chunk group found"))?
      };

      for dep in dependencies.iter() {
        let module = module_graph
          .module_by_dependency(dep)
          .ok_or_else(|| anyhow::format_err!("no module found"))?;
        compilation.chunk_graph.add_module(module.module_identifier);

        input_entrypoints_and_modules
          .entry(entrypoint.ukey)
          .or_default()
          .push(module.module_identifier);

        compilation.chunk_graph.connect_chunk_and_entry_module(
          chunk.ukey,
          module.module_identifier,
          entrypoint.ukey,
        );
      }
    }

    for (name, entry_data) in entries.iter() {
      let options = &entry_data.options;

      if let Some(runtime) = &options.runtime {
        let ukey = compilation
          .entrypoints
          .get(name)
          .ok_or_else(|| anyhow!("no entrypoints found"))?;

        let entry_point = compilation
          .chunk_group_by_ukey
          .get_mut(ukey)
          .ok_or_else(|| anyhow!("no chunk group found"))?;

        let chunk = match compilation.named_chunks.get(runtime) {
          Some(ukey) => compilation
            .chunk_by_ukey
            .get_mut(ukey)
            .ok_or_else(|| anyhow!("no chunk found"))?,
          None => {
            let chunk = Compilation::add_named_chunk(
              runtime.to_string(),
              runtime.to_string(),
              &mut compilation.chunk_by_ukey,
              &mut compilation.named_chunks,
            );
            compilation.chunk_graph.add_chunk(chunk.ukey);
            chunk
          }
        };

        entry_point.unshift_chunk(chunk);
        entry_point.set_runtime_chunk(chunk.ukey);
      }
    }
    Ok(input_entrypoints_and_modules)
  }

  pub fn split(mut self) -> Result<()> {
    let input_entrypoints_and_modules = self.prepare_input_entrypoints_and_modules()?;

    for (chunk_group, modules) in input_entrypoints_and_modules {
      let chunk_group = self
        .compilation
        .chunk_group_by_ukey
        .get(&chunk_group)
        .ok_or_else(|| anyhow::format_err!("no chunk group found"))?;
      // We could assume that the chunk group is an entrypoint and must have one chunk, which is entry chunk.
      // TODO: we need a better and safe way to ensure this.
      let chunk = chunk_group.get_entry_point_chunk();
      for module in modules {
        self.queue.push(QueueItem {
          action: QueueAction::AddAndEnter,
          chunk,
          chunk_group: chunk_group.ukey,
          module_identifier: module,
        });
      }
    }
    self.queue.reverse();

    tracing::trace!("--- process_queue start ---");
    while !self.queue.is_empty() || !self.queue_delayed.is_empty() {
      self.process_queue();
      if self.queue.is_empty() {
        self.queue = std::mem::take(&mut self.queue_delayed);
      }
    }
    tracing::trace!("--- process_queue end ---");

    // Optmize to remove duplicated module which is safe

    let mut modules_to_be_removed_in_chunk =
      HashMap::new() as HashMap<ChunkUkey, HashSet<ModuleIdentifier>>;

    for chunk in self.compilation.chunk_by_ukey.values() {
      for module in self
        .compilation
        .chunk_graph
        .get_chunk_modules(&chunk.ukey, &self.compilation.module_graph)
      {
        let belong_to_chunks = self
          .compilation
          .chunk_graph
          .get_modules_chunks(&module.module_identifier)
          .clone();

        let has_superior = belong_to_chunks.iter().any(|maybe_superior_chunk| {
          self
            .chunk_relation_graph
            .contains_edge(chunk.ukey, *maybe_superior_chunk)
        });

        if has_superior {
          modules_to_be_removed_in_chunk
            .entry(chunk.ukey)
            .or_default()
            .insert(module.module_identifier);
        }

        tracing::trace!(
          "module {} in chunk {:?} has_superior {:?}",
          module.module_identifier,
          chunk.id,
          has_superior
        );
      }
    }

    for (chunk, modules) in modules_to_be_removed_in_chunk {
      for module in modules {
        self
          .compilation
          .chunk_graph
          .disconnect_chunk_and_module(&chunk, &module);
      }
    }

    for chunk_group in self.compilation.chunk_group_by_ukey.values() {
      if let ChunkGroupKind::Entrypoint = chunk_group.kind {
        for chunk_ukey in chunk_group.chunks.iter() {
          self
            .compilation
            .chunk_by_ukey
            .entry(*chunk_ukey)
            .and_modify(|chunk| {
              chunk.runtime.extend(
                chunk_group
                  .runtime
                  .clone()
                  .expect("ChunkGroupKind::Entrypoint should has runtime"),
              );
            });
        }
      }
    }

    Ok(())
  }

  fn process_queue(&mut self) {
    tracing::trace!("process_queue");
    while let Some(queue_item) = self.queue.pop() {
      match queue_item.action {
        QueueAction::AddAndEnter => self.add_and_enter_module(&queue_item),
        QueueAction::_Enter => self.enter_module(&queue_item),
        QueueAction::_ProcessModule => self.process_module(&queue_item),
        QueueAction::Leave => self.leave_module(&queue_item),
      }
    }
  }

  fn add_and_enter_module(&mut self, item: &QueueItem) {
    tracing::trace!("add_and_enter_module {:?}", item);
    if self
      .compilation
      .chunk_graph
      .is_module_in_chunk(&item.module_identifier, item.chunk)
    {
      return;
    }

    self
      .compilation
      .chunk_graph
      .add_module(item.module_identifier);

    self
      .compilation
      .chunk_graph
      .connect_chunk_and_module(item.chunk, item.module_identifier);
    self.enter_module(item)
  }

  fn enter_module(&mut self, item: &QueueItem) {
    tracing::trace!("enter_module {:?}", item);
    let chunk_group = self
      .compilation
      .chunk_group_by_ukey
      .get_mut(&item.chunk_group)
      .expect("chunk group not found");

    if chunk_group
      .module_pre_order_indices
      .get(&item.module_identifier)
      .is_none()
    {
      chunk_group
        .module_pre_order_indices
        .insert(item.module_identifier, chunk_group.next_pre_order_index);
      chunk_group.next_pre_order_index += 1;
    }

    {
      let mut module = self
        .compilation
        .module_graph
        .module_graph_module_by_identifier_mut(&item.module_identifier)
        .expect("No module found");

      if module.pre_order_index.is_none() {
        module.pre_order_index = Some(self.next_free_module_pre_order_index);
        self.next_free_module_pre_order_index += 1;
      }
    }

    self.queue.push(QueueItem {
      action: QueueAction::Leave,
      ..item.clone()
    });
    self.process_module(item)
  }

  fn leave_module(&mut self, item: &QueueItem) {
    tracing::trace!("leave_module {:?}", item);
    let chunk_group = self
      .compilation
      .chunk_group_by_ukey
      .get_mut(&item.chunk_group)
      .expect("chunk group not found");

    if chunk_group
      .module_post_order_indices
      .get(&item.module_identifier)
      .is_none()
    {
      chunk_group
        .module_post_order_indices
        .insert(item.module_identifier, chunk_group.next_post_order_index);
      chunk_group.next_post_order_index += 1;
    }

    let mut module = self
      .compilation
      .module_graph
      .module_graph_module_by_identifier_mut(&item.module_identifier)
      .expect("no module found");

    if module.post_order_index.is_none() {
      module.post_order_index = Some(self.next_free_module_post_order_index);
      self.next_free_module_post_order_index += 1;
    }
  }

  fn process_module(&mut self, item: &QueueItem) {
    tracing::trace!("process_module {:?}", item);
    let mgm = self
      .compilation
      .module_graph
      .module_graph_module_by_identifier(&item.module_identifier)
      .expect("no module found");

    for dep_mgm in mgm
      .depended_modules(&self.compilation.module_graph)
      .into_iter()
      .rev()
    {
      self.queue.push(QueueItem {
        action: QueueAction::AddAndEnter,
        chunk: item.chunk,
        chunk_group: item.chunk_group,
        module_identifier: dep_mgm.module_identifier,
      });
    }

    for dyn_dep_mgm in mgm
      .dynamic_depended_modules(&self.compilation.module_graph)
      .into_iter()
      .rev()
    {
      let is_already_split_module = self
        .split_point_modules
        .contains(&dyn_dep_mgm.module_identifier);
      if is_already_split_module {
        continue;
      } else {
        self
          .split_point_modules
          .insert(dyn_dep_mgm.module_identifier);
      }

      let chunk = Compilation::add_named_chunk(
        uri_to_chunk_name(
          &self.compilation.options.context.to_string_lossy(),
          // TODO: change to chunk group name
          &dyn_dep_mgm.module_identifier,
        ),
        uri_to_chunk_name(
          &self.compilation.options.context.to_string_lossy(),
          // TODO: change to chunk group name
          &dyn_dep_mgm.module_identifier,
        ),
        &mut self.compilation.chunk_by_ukey,
        &mut self.compilation.named_chunks,
      );
      self.compilation.chunk_graph.add_chunk(chunk.ukey);
      self
        .chunk_relation_graph
        .add_edge(chunk.ukey, item.chunk, ());

      self
        .compilation
        .chunk_graph
        .split_point_module_identifier_to_chunk_ukey
        .insert(dyn_dep_mgm.module_identifier, chunk.ukey);

      let mut chunk_group = ChunkGroup::new(ChunkGroupKind::Normal, None);
      let item_chunk_group = self
        .compilation
        .chunk_group_by_ukey
        .get_mut(&item.chunk_group)
        .expect("chunk group not found");
      item_chunk_group.children.insert(chunk_group.ukey);
      chunk_group.parents.insert(item_chunk_group.ukey);

      chunk_group.connect_chunk(chunk);

      let chunk_group = {
        let ukey = chunk_group.ukey;
        self
          .compilation
          .chunk_group_by_ukey
          .insert(ukey, chunk_group);

        self.compilation.chunk_group_by_ukey.get(&ukey).unwrap()
      };

      self.queue_delayed.push(QueueItem {
        action: QueueAction::AddAndEnter,
        chunk: chunk.ukey,
        chunk_group: chunk_group.ukey,
        module_identifier: dyn_dep_mgm.module_identifier,
      });
    }
  }
}

#[derive(Debug, Clone)]
struct QueueItem {
  action: QueueAction,
  chunk_group: ChunkGroupUkey,
  chunk: ChunkUkey,
  module_identifier: ModuleIdentifier,
}

#[derive(Debug, Clone)]
enum QueueAction {
  AddAndEnter,
  _Enter,
  _ProcessModule,
  Leave,
}

// struct chunkGroupInfoMap {}
