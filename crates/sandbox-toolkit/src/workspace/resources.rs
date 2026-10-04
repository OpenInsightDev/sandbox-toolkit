use std::sync::Arc;

use crate::mcp;
use crate::skill::Skills;

use super::Metadata;

#[derive(Clone)]
pub struct Resources {
    pub mcps: Arc<mcp::Runtime>,
    pub skills: Skills,
}

impl Resources {
    pub async fn new(metadata: &Metadata) -> Result<Self, mcp::Error> {
        let mcps = Arc::new(mcp::Runtime::new(&metadata.root).await?);
        let skills = Skills::new(&metadata.id, &metadata.root);

        Ok(Self { mcps, skills })
    }
}
