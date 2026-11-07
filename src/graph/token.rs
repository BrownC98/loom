const LAUNCH_MARK: char = '-';
const NODE_BYTES: usize = 16;
const LAUNCH_BYTES: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeToken(String);

impl NodeToken {
    pub fn fresh() -> Result<NodeToken, String> {
        Self::launch_of(&hex(NODE_BYTES)?)
    }

    pub fn next_launch(&self) -> Result<NodeToken, String> {
        Self::launch_of(self.node())
    }

    pub fn node(&self) -> &str {
        node_part(&self.0)
    }

    pub fn launch_prefix(&self) -> String {
        launch_prefix(self.node())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(super) fn stored(token: String) -> NodeToken {
        NodeToken(token)
    }

    fn launch_of(node: &str) -> Result<NodeToken, String> {
        Ok(NodeToken(format!(
            "{node}{LAUNCH_MARK}{}",
            hex(LAUNCH_BYTES)?
        )))
    }
}

pub(super) fn node_part(token: &str) -> &str {
    token
        .split_once(LAUNCH_MARK)
        .map_or(token, |(node, _)| node)
}

pub(super) fn launch_prefix(node: &str) -> String {
    format!("{node}{LAUNCH_MARK}")
}

fn hex(bytes: usize) -> Result<String, String> {
    let mut random = vec![0u8; bytes];
    getrandom::fill(&mut random).map_err(|e| e.to_string())?;
    Ok(random.iter().map(|b| format!("{b:02x}")).collect())
}
