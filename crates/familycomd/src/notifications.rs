//! Desktop notification manager.
//!
//! Sends native desktop notifications when messages arrive and the TUI
//! is not actively connected. Uses `notify-rust` which provides a
//! unified API across platforms:
//!
//! - **Linux**: D-Bus notifications (works with GNOME, KDE, XFCE, etc.)
//! - **macOS**: NSUserNotification (via mac-notification-sys internally)
//!
//! # Rate Limiting
//!
//! To avoid spamming the user with notifications when many messages
//! arrive at once, we limit to at most one notification per second.

use std::time::{Duration, Instant};
use tracing::{debug, error};

/// Minimum time between notifications to prevent spam.
const MIN_NOTIFICATION_INTERVAL: Duration = Duration::from_secs(1);

/// Manages desktop notification delivery.
pub struct NotificationManager {
    /// When the last notification was shown.
    last_notification: Option<Instant>,
    /// Whether notifications are enabled.
    enabled: bool,
}

impl NotificationManager {
    /// Creates a new notification manager with notifications enabled.
    pub fn new() -> Self {
        Self {
            last_notification: None,
            enabled: true,
        }
    }

    /// Sends a notification for a new incoming message.
    ///
    /// Respects rate limiting — if another notification was shown less
    /// than 1 second ago, this call is silently ignored.
    ///
    /// # Arguments
    ///
    /// * `sender_name` - Display name of the peer who sent the message
    /// * `preview` - A preview of the message content (first ~100 chars)
    pub fn notify_new_message(&mut self, sender_name: &str, preview: &str) {
        if !self.enabled {
            return;
        }

        // Rate limiting: skip if we sent a notification too recently
        if let Some(last) = self.last_notification {
            if last.elapsed() < MIN_NOTIFICATION_INTERVAL {
                debug!("notification rate-limited, skipping");
                return;
            }
        }

        // Truncate preview to avoid overly long notifications
        let truncated_preview = if preview.len() > 100 {
            format!("{}...", &preview[..preview.floor_char_boundary(97)])
        } else {
            preview.to_string()
        };

        // Send the notification using notify-rust
        let result = notify_rust::Notification::new()
            .summary(&format!("FamilyCom - {sender_name}"))
            .body(&truncated_preview)
            .timeout(notify_rust::Timeout::Milliseconds(5000))
            .show();

        match result {
            Ok(_) => {
                debug!(sender = sender_name, "notification sent");
                self.last_notification = Some(Instant::now());
            }
            Err(e) => {
                error!(error = %e, "failed to send notification");
            }
        }
    }

    /// Enables or disables notifications.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }
}
