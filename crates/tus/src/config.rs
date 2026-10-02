/// URL path the upload endpoints are mounted at, mirroring tusd's `BasePath`.
/// An empty value mounts the collection at `/` and resources at `/{upload_id}`.
#[derive(Debug, Clone)]
pub struct Config {
    pub base_path: String,
    /// `0` disables the limit.
    pub max_size: u64,
    pub disable_termination: bool,
    pub disable_concatenation: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_path: "/upload".to_owned(),
            max_size: 0,
            disable_termination: false,
            disable_concatenation: false,
        }
    }
}

impl Config {
    /// Normalizes to a leading slash and no trailing slash.
    pub(crate) fn normalize(&mut self) {
        let trimmed = self.base_path.trim_matches('/');
        self.base_path = if trimmed.is_empty() {
            String::new()
        } else {
            format!("/{trimmed}")
        };
    }
}
