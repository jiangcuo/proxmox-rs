use proxmox_schema::api_types::{SAFE_ID_FORMAT, SINGLE_LINE_COMMENT_FORMAT};
use proxmox_schema::{Schema, StringSchema};

pub const EMAIL_SCHEMA: Schema = StringSchema::new("E-Mail Address.")
    .format(&SINGLE_LINE_COMMENT_FORMAT)
    .min_length(2)
    .max_length(64)
    .schema();

pub const BACKEND_NAME_SCHEMA: Schema = StringSchema::new("Notification backend name.")
    .format(&SAFE_ID_FORMAT)
    .min_length(3)
    .max_length(32)
    .schema();

pub const ENTITY_NAME_SCHEMA: Schema =
    StringSchema::new("Name schema for endpoints, filters and groups")
        .format(&SAFE_ID_FORMAT)
        .min_length(2)
        .max_length(32)
        .schema();
