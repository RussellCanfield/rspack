use rspack_core::Plugin;

#[derive(Debug)]
pub struct WorkerPlugin {}

#[async_trait::async_trait]
impl Plugin for WorkerPlugin {}
