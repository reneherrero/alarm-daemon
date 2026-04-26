use alarm_daemon_client::AlarmDaemonClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = AlarmDaemonClient::connect_session().await?;

    let sounds = client.list_sounds().await?;
    println!("available sounds: {sounds:?}");

    if let Some(sound_id) = sounds.first() {
        client.arm(sound_id).await?;
        println!("armed with {sound_id}");
        println!("status: {}", client.status().await?);
        println!("current sound: {:?}", client.current_sound().await?);

        client.disarm().await?;
        println!("disarmed");
    } else {
        println!("no sounds available");
    }

    Ok(())
}
