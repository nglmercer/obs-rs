use obs_rs_output::OutputProfile;

use super::destination::ProductionDestination;
use super::{GStreamerError, MAX_WEBRTC_SIGNALING_BYTES};

/// Application-driven WebRTC signaling lifecycle. Network exchange remains in
/// the application; the media adapter never logs or owns signaling credentials.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WebRtcSignalingState {
    AwaitingLocalDescription,
    AwaitingRemoteDescription,
    Connecting,
    Connected,
    Retrying,
    Failed,
    Closed,
}

/// Bounded state machine joining an application signaling channel to WebRTC.
pub struct WebRtcSignalingSession {
    state: WebRtcSignalingState,
    retries: u32,
}

impl WebRtcSignalingSession {
    /// Creates a signaling session after validating its typed destination.
    ///
    /// # Errors
    ///
    /// Rejects non-WebRTC destinations and invalid signaling endpoints.
    pub fn new(destination: &ProductionDestination) -> Result<Self, GStreamerError> {
        destination.validate_for(OutputProfile::web_rtc_vp8_opus())?;
        Ok(Self {
            state: WebRtcSignalingState::AwaitingLocalDescription,
            retries: 0,
        })
    }

    #[must_use]
    pub const fn state(&self) -> WebRtcSignalingState {
        self.state
    }

    #[must_use]
    pub const fn retries(&self) -> u32 {
        self.retries
    }

    /// Marks a bounded local SDP description ready for application delivery.
    ///
    /// # Errors
    ///
    /// Rejects invalid state, empty/oversized SDP, and embedded NUL bytes.
    pub fn local_description_ready(&mut self, sdp: &str) -> Result<(), GStreamerError> {
        self.require_state(WebRtcSignalingState::AwaitingLocalDescription)?;
        validate_sdp(sdp)?;
        self.state = WebRtcSignalingState::AwaitingRemoteDescription;
        Ok(())
    }

    /// Accepts a bounded remote SDP answer supplied by the application.
    ///
    /// # Errors
    ///
    /// Rejects invalid state or malformed/oversized SDP.
    pub fn remote_description_received(&mut self, sdp: &str) -> Result<(), GStreamerError> {
        self.require_state(WebRtcSignalingState::AwaitingRemoteDescription)?;
        validate_sdp(sdp)?;
        self.state = WebRtcSignalingState::Connecting;
        Ok(())
    }

    /// Marks ICE/media connectivity established.
    ///
    /// # Errors
    ///
    /// Rejects a connection notification in another lifecycle state.
    pub fn connected(&mut self) -> Result<(), GStreamerError> {
        self.require_state(WebRtcSignalingState::Connecting)?;
        self.state = WebRtcSignalingState::Connected;
        Ok(())
    }

    /// Starts one bounded retry, requiring a fresh offer/answer exchange.
    ///
    /// # Errors
    ///
    /// Rejects retries after close/failure or after `maximum_retries`.
    pub fn retry(&mut self, maximum_retries: u32) -> Result<(), GStreamerError> {
        if matches!(
            self.state,
            WebRtcSignalingState::Closed | WebRtcSignalingState::Failed
        ) || self.retries >= maximum_retries
        {
            self.state = WebRtcSignalingState::Failed;
            return Err(GStreamerError::Native(
                "WebRTC signaling retry limit reached".to_owned(),
            ));
        }
        self.state = WebRtcSignalingState::Retrying;
        self.retries = self.retries.saturating_add(1);
        self.state = WebRtcSignalingState::AwaitingLocalDescription;
        Ok(())
    }

    pub const fn close(&mut self) {
        self.state = WebRtcSignalingState::Closed;
    }

    fn require_state(&self, expected: WebRtcSignalingState) -> Result<(), GStreamerError> {
        if self.state == expected {
            Ok(())
        } else {
            Err(GStreamerError::Native(format!(
                "WebRTC signaling is {:?}, expected {expected:?}",
                self.state
            )))
        }
    }
}

fn validate_sdp(sdp: &str) -> Result<(), GStreamerError> {
    if sdp.is_empty() || sdp.len() > MAX_WEBRTC_SIGNALING_BYTES || sdp.contains('\0') {
        Err(GStreamerError::InvalidEndpoint(
            "WebRTC SDP is empty, oversized, or contains NUL".to_owned(),
        ))
    } else {
        Ok(())
    }
}
