use proxmox_api_macro::api;

use failure::Error;

#[api(
    input: {
        properties: {
            message: {
                description: "The message to print",
            }
        }
    }
)]
/// Print the given message.
pub fn hello(message: String) -> Result<(), Error> {
    println!("Hello there. {}", message);
    Ok(())
}
