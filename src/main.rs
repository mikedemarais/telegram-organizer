use std::env;
use log::info;
use dotenv::dotenv;

mod telegram;
mod database;
mod ai;
mod scheduler;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load environment variables from .env file
    dotenv().ok();

    // Initialize simple logger (prints info/debug messages to stdout)
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Info)
        .init()
        .unwrap();

    // Load Telegram API credentials from environment variables
    let api_id: u32 = env::var("TG_ID")
        .expect("Please set TG_ID environment variable to your Telegram API ID")
        .parse()
        .expect("TG_ID must be an integer (your Telegram API ID)");
    let api_hash: String = env::var("TG_HASH")
        .expect("Please set TG_HASH environment variable to your Telegram API hash");

    // Initialize SQLite database (creates file and tables if not exist)
    let db_path = "telegram_monitor.db";
    let mut conn = database::init_db(db_path)?;

    // Connect to Telegram (establish session, authenticate if needed)
    let client = telegram::connect(api_id, &api_hash, "telegram.session").await?;
    info!("Telegram client connected and authorized.");

    // Check for "--review" CLI argument
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && args[1] == "--review" {
        // If review flag, output the stored categorized chats and urgent messages
        database::print_report(&conn)?;
    } else {
        // Run the periodic monitoring loop (every 30 minutes)
        info!("Starting monitoring loop. Press Ctrl+C to stop.");
        scheduler::run_schedule(&client, &mut conn).await?;
    }

    Ok(())
} 