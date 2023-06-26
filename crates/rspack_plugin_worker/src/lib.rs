use rspack_core::{BoxPlugin, ChunkLoading};
use rspack_plugin_runtime::enable_chunk_loading_plugin;

pub fn worker_plugin(worker_chunk_loading: ChunkLoading, plugins: &mut Vec<BoxPlugin>) {
  if let ChunkLoading::Enable(loading_type) = worker_chunk_loading {
    enable_chunk_loading_plugin(loading_type, plugins);
  }
}
