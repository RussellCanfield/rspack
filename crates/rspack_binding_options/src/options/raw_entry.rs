use napi_derive::napi;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
#[napi(object)]
pub struct RawEntryDescription {
  pub import: Vec<String>,
  pub runtime: Option<String>,
  pub chunk_loading: Option<String>,
  pub async_chunks: Option<bool>,
  pub public_path: Option<String>,
  pub base_uri: Option<String>,
}
