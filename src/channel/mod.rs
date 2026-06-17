//! Generic communication channel abstractions.

pub mod handler;
pub mod registry;
pub mod traits;
pub mod types;

pub use handler::{ChannelMessageHandler, ChannelMessageHandlerInput};
pub use registry::ChannelRegistry;
pub use traits::{ChannelIngress, ChannelService, ChannelTypingIndicator};
pub use types::{
    AttachmentInfo, ChannelAccessPolicy, ChannelConfig, ChannelId, ChannelInboundEvent,
    ConversationAddress, ConversationAddressPattern, InboundMessage, MessageFormat,
    OutboundMessage, SenderIdentity,
};
