# Telegram Chat Organizer

A Rust-based system that silently monitors Telegram group chats using the MTProto API. It fetches messages without marking them as read, analyzes content using a local Ollama AI model, and stores insights in SQLite.

## Features

- Silent monitoring of Telegram group chats and channels
- Local AI analysis using Ollama
- Automatic categorization of chats
- Duplicate chat detection
- Urgent message identification
- SQLite storage for persistence
- No read receipts - completely passive monitoring

## Prerequisites

- Rust (latest stable version)
- SQLite
- [Ollama](https://ollama.ai/) installed and running locally
- Telegram API credentials (see below)

## Setup

1. **Install Rust and Cargo**
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. **Install Ollama**
   Follow the installation instructions at [ollama.ai](https://ollama.ai/download)

3. **Clone the repository**
   ```bash
   git clone https://github.com/yourusername/telegram-organizer.git
   cd telegram-organizer
   ```

4. **Get Telegram API Credentials**
   - Go to https://my.telegram.org/apps
   - Create a new application
   - Note down your `api_id` and `api_hash`

5. **Create .env file**
   Create a `.env` file in the project root with:
   ```
   TG_ID=your_api_id
   TG_HASH=your_api_hash
   OLLAMA_MODEL=llama2:latest  # or your preferred model
   ```

6. **Build the project**
   ```bash
   cargo build --release
   ```

## Usage

### First Run
The first time you run the program, it will prompt for Telegram authentication:
```bash
./target/release/telegram-organizer
```

Follow the prompts to:
1. Enter your phone number
2. Enter the verification code sent via Telegram
3. Enter your 2FA password (if enabled)

### Regular Operation
The program operates in two modes:

1. **Monitor Mode (Default)**
   ```bash
   ./target/release/telegram-organizer
   ```
   - Runs continuously, checking for new messages every 30 minutes
   - Analyzes chat content using Ollama
   - Stores results in SQLite database

2. **Review Mode**
   ```bash
   ./target/release/telegram-organizer --review
   ```
   - Prints a report of all monitored chats
   - Shows categories and urgent messages
   - Highlights duplicate chat topics

### Output Files
- `telegram.session`: Stores Telegram session (auto-generated)
- `telegram_monitor.db`: SQLite database with chat history and analysis

## Security Considerations

- Keep your `.env` file secure and never commit it
- The `telegram.session` file contains sensitive session data
- The SQLite database contains message history
- All AI processing is done locally via Ollama

## Performance Notes

- Messages are fetched in batches of 100 (Telegram API limit)
- AI analysis uses a context window of 20 messages
- Database operations use transactions for efficiency
- 30-minute scheduler interval balances freshness and API limits

## Troubleshooting

### Authentication Issues
- Delete `telegram.session` and retry
- Verify API credentials in `.env`

### Database Errors
- Ensure SQLite is installed
- Check file permissions

### AI Analysis Issues
- Verify Ollama is running (`curl http://localhost:11434/api/version`)
- Check if the specified model is available in Ollama

### Telegram API Errors

#### PEER_ID_INVALID / CHAT_ID_INVALID / CHANNEL_INVALID
These errors occur when trying to fetch messages from chats that:
1. The account no longer has access to
2. Have been deleted or archived
3. Have invalid access hashes

To resolve:
1. **Clean Session**: Delete `telegram.session` and restart to refresh all chat access tokens
2. **Verify Permissions**: Ensure your Telegram account still has access to the chats
3. **Update Access Hashes**: The program will automatically update access hashes on next login
4. **Filter Chats**: You can modify `src/telegram.rs` to skip problematic chats

#### Rate Limiting
If you see many errors in succession:
1. The program automatically handles rate limits by waiting between requests
2. Default interval (30 minutes) helps avoid hitting API limits
3. Consider increasing the interval in `scheduler.rs` if needed

#### AI Analysis Errors
If you see "AI generation failed: Error in Ollama":
1. Verify Ollama is running and responsive
2. Check the model specified in `.env` exists
3. Monitor Ollama logs for specific error messages
4. Consider increasing batch size or reducing context window

### Common Solutions

1. **Reset Session**
   ```bash
   rm telegram.session
   rm telegram_monitor.db
   ./target/release/telegram-organizer
   ```

2. **Verify API Access**
   ```bash
   # Test Telegram API credentials
   echo $TG_ID
   echo $TG_HASH
   
   # Test Ollama
   curl http://localhost:11434/api/version
   ```

3. **Debug Mode**
   Run with more verbose logging:
   ```bash
   RUST_LOG=debug ./target/release/telegram-organizer
   ```

4. **Clean Start**
   If having persistent issues:
   ```bash
   # Stop any running instances
   pkill telegram-organizer
   
   # Remove all generated files
   rm telegram.session
   rm telegram_monitor.db
   
   # Rebuild and start fresh
   cargo clean
   cargo build --release
   ./target/release/telegram-organizer
   ```

## Contributing

1. Fork the repository
2. Create your feature branch
3. Commit your changes
4. Push to the branch
5. Create a new Pull Request

## License

This project is licensed under the MIT License - see the LICENSE file for details. 