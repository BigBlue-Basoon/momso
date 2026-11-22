//! Browser-independent operations shared by desktop and server adapters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    pub id: String,
    pub role: String,
    pub name: String,
    pub kind: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageState {
    pub origin: String,
    pub title: String,
    pub text: String,
    pub elements: Vec<Element>,
    pub data_provenance: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DriverError {
    Unavailable,
    Blocked(String),
    HumanRequired(&'static str),
    Refused(&'static str),
    TargetChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserPermission {
    Ask,
    Automatic,
    Plan,
    AutoReview,
}

impl BrowserPermission {
    pub fn from_provider_mode(provider: &str, mode: &str) -> Self {
        match (provider, mode) {
            ("codex", "Full access") | ("claude", "bypassPermissions") | ("gemini", "yolo") => {
                Self::Automatic
            }
            ("codex", "Approve for me") | ("claude", "auto") => Self::AutoReview,
            (_, "plan") => Self::Plan,
            _ => Self::Ask,
        }
    }
}

pub trait BrowserEngine: Send {
    fn navigate(&mut self, url: &str) -> Result<PageState, DriverError>;
    fn read(&mut self) -> Result<PageState, DriverError>;
    fn scroll(&mut self, _direction: &str, _origin: &str) -> Result<(), DriverError> {
        Err(DriverError::Unavailable)
    }
    fn click_checked(
        &mut self,
        id: &str,
        name: &str,
        origin: Option<&str>,
    ) -> Result<(), DriverError>;
    fn fill_checked(
        &mut self,
        id: &str,
        text: &str,
        name: Option<&str>,
        origin: Option<&str>,
    ) -> Result<(), DriverError>;
}
