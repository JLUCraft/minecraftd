//! HTTP boundary allowing hosts to retain their proxy and TLS configuration.
use crate::resource::ResourceError;
use serde_json::Value;
use std::collections::HashMap;
#[async_trait::async_trait]
pub trait ResourceTransport: Send + Sync {
    async fn request_json(
        &self,
        url: &str,
        query: &HashMap<String, String>,
        body: Option<&Value>,
        api_key: Option<&str>,
    ) -> Result<Value, ResourceError>;
}
#[async_trait::async_trait]
impl ResourceTransport for reqwest::Client {
    async fn request_json(
        &self,
        url: &str,
        query: &HashMap<String, String>,
        body: Option<&Value>,
        api_key: Option<&str>,
    ) -> Result<Value, ResourceError> {
        let mut request = body
            .map_or_else(|| self.get(url), |body| self.post(url).json(body))
            .query(query);
        if let Some(key) = api_key {
            request = request.header("x-api-key", key);
        }
        Ok(request.send().await?.error_for_status()?.json().await?)
    }
}

#[async_trait::async_trait]
impl<C: ResourceTransport> ResourceTransport for &C {
    async fn request_json(
        &self,
        url: &str,
        query: &HashMap<String, String>,
        body: Option<&Value>,
        api_key: Option<&str>,
    ) -> Result<Value, ResourceError> {
        (**self).request_json(url, query, body, api_key).await
    }
}
