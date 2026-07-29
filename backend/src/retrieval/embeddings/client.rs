use super::*;

#[derive(Clone)]
pub(in crate::retrieval) struct EmbeddingClient {
    pub(in crate::retrieval) http: reqwest::Client,
    pub(in crate::retrieval) service_url: String,
    pub(in crate::retrieval) model: String,
    pub(in crate::retrieval) dimensions: i32,
}

/// Embedding servers cap the number of inputs per request — text-embeddings-
/// inference defaults to `--max-client-batch-size 32` and returns HTTP 422
/// ("batch size N > maximum allowed batch size 32") above it. The indexer's
/// logical `embedding_batch_size` is a DB-processing granularity that may be
/// larger, so each embedding call is split into HTTP sub-batches no larger than
/// this. 32 is TEI's default and a safe lower bound for other servers.
const EMBEDDING_HTTP_MAX_BATCH: usize = 32;

impl EmbeddingClient {
    pub(in crate::retrieval) async fn embed_one(
        &self,
        text: &str,
    ) -> Result<Vec<f32>, RetrievalError> {
        let mut embeddings = self.embed_batch(&[text.to_string()]).await?;
        embeddings
            .pop()
            .ok_or_else(|| RetrievalError::unavailable("embedding response was empty"))
    }

    pub(in crate::retrieval) async fn embed_batch(
        &self,
        input: &[String],
    ) -> Result<Vec<Vec<f32>>, RetrievalError> {
        let mut out = Vec::with_capacity(input.len());
        for chunk in input.chunks(EMBEDDING_HTTP_MAX_BATCH) {
            out.extend(self.embed_http_batch(chunk).await?);
        }
        Ok(out)
    }

    /// One embedding request for an input batch already within the server's
    /// per-request limit (see [`EMBEDDING_HTTP_MAX_BATCH`]). Retries transient
    /// failures up to three times.
    async fn embed_http_batch(&self, input: &[String]) -> Result<Vec<Vec<f32>>, RetrievalError> {
        if input.is_empty() {
            return Ok(Vec::new());
        }
        let mut last_error = None;
        let endpoint = format!("{}/v1/embeddings", self.service_url.trim_end_matches('/'));
        for attempt in 1..=3 {
            let result = self
                .http
                .post(&endpoint)
                .json(&EmbeddingRequest {
                    model: self.model.as_str(),
                    input,
                    encoding_format: "float",
                    dimensions: Some(self.dimensions),
                })
                .send()
                .await
                .map_err(|err| {
                    RetrievalError::unavailable(format!("embedding request failed: {err}"))
                })
                .and_then(|response| {
                    response.error_for_status().map_err(|err| {
                        RetrievalError::unavailable(format!("embedding request failed: {err}"))
                    })
                });
            match result {
                Ok(response) => {
                    let response = response.json::<EmbeddingResponse>().await.map_err(|err| {
                        RetrievalError::unavailable(format!("invalid embedding response: {err}"))
                    })?;
                    return validate_embedding_response(response, input.len());
                }
                Err(err) => {
                    last_error = Some(err);
                    if attempt < 3 {
                        tokio::time::sleep(Duration::from_millis(100 * attempt)).await;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| RetrievalError::unavailable("embedding request failed")))
    }
}

fn validate_embedding_response(
    response: EmbeddingResponse,
    input_len: usize,
) -> Result<Vec<Vec<f32>>, RetrievalError> {
    let mut data = response.data;
    data.sort_by_key(|item| item.index);
    if data.len() != input_len {
        return Err(RetrievalError::unavailable(format!(
            "embedding response length mismatch: expected {}, got {}",
            input_len,
            data.len()
        )));
    }
    Ok(data.into_iter().map(|item| item.embedding).collect())
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    encoding_format: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<i32>,
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingItem>,
}

#[derive(Deserialize)]
struct EmbeddingItem {
    index: i32,
    embedding: Vec<f32>,
}
