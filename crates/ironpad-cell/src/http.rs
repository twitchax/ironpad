//! HTTP helper functions for making network requests from cells.

/// Perform a GET request and return the response body as a String.
pub async fn get(url: &str) -> Result<String, String> {
    reqwest::get(url)
        .await
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())
}

/// Perform a GET request and deserialize the JSON response.
pub async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, String> {
    reqwest::get(url)
        .await
        .map_err(|e| e.to_string())?
        .json::<T>()
        .await
        .map_err(|e| e.to_string())
}

/// Perform a POST request with a JSON body.
pub async fn post_json<B: serde::Serialize, R: serde::de::DeserializeOwned>(
    url: &str,
    body: &B,
) -> Result<R, String> {
    reqwest::Client::new()
        .post(url)
        .json(body)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<R>()
        .await
        .map_err(|e| e.to_string())
}
