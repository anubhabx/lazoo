use windows::{
    Data::Xml::Dom::XmlDocument,
    UI::Notifications::{ToastNotification, ToastNotificationManager},
    core::HSTRING,
};

use crate::RuntimeError;

#[derive(Clone, Copy, Debug, Default)]
pub struct NativeNotifier;

impl NativeNotifier {
    pub fn notify(&self, title: &str, message: &str) -> Result<(), RuntimeError> {
        let xml = format!(
            "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
            escape_xml(title),
            escape_xml(message)
        );
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(xml))?;
        let notification = ToastNotification::CreateToastNotification(&document)?;
        let notifier =
            ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from("Lazoo"))?;
        notifier.Show(&notification)?;
        Ok(())
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::escape_xml;

    #[test]
    fn notification_text_is_xml_escaped() {
        assert_eq!(escape_xml("A < B & \"C\""), "A &lt; B &amp; &quot;C&quot;");
    }
}
