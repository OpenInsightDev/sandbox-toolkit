use std::path::Path;
use std::sync::Arc;

use crate::mcp;

pub struct Resources {
    pub mcps: Arc<mcp::Runtime>,
}

impl Resources {
    pub async fn new(root: &Path) -> Result<Self, mcp::Error> {
        let mcps = Arc::new(mcp::Runtime::new(root).await?);
        Ok(Self { mcps })
    }
}
