#[cfg(any(feature = "sendmail", feature = "smtp"))]
pub(crate) mod mail;

#[cfg(any(
    feature = "dingtalk",
    feature = "feishu",
    feature = "wecom",
    feature = "sms"
))]
pub(crate) mod http;
