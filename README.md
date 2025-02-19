# Telegram Chat Organizer

A Rust-based system that silently monitors Telegram group chats using the MTProto API. It fetches messages without marking them as read, analyzes content using a local Ollama AI model, and stores insights using libSQL.

## Features

- Silent monitoring of Telegram group chats and channels
- Local AI analysis using Ollama
- Automatic categorization of chats
- Duplicate chat detection
- Urgent message identification
- libSQL storage with vector embedding support
- No read receipts – completely passive monitoring

## Prerequisites

- Rust (latest stable version)
- libSQL (the project uses libSQL instead of SQLite)
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
   - Analyzes chat content via Ollama
   - Stores results in the libSQL database

2. **Review Mode**
   ```bash
   ./target/release/telegram-organizer --review
   ```
   - Prints a report of all monitored chats
   - Shows categories and urgent messages
   - Highlights duplicate chat topics

## Output Files

- `telegram.session`: Stores Telegram session (auto-generated)
- `telegram_monitor_new.db`: libSQL database containing chat history, AI analysis, and vector embeddings

## Security Considerations

- Keep your `.env` file secure and never commit it
- The `telegram.session` file contains sensitive session data
- The libSQL database contains message history and embeddings
- All AI processing is performed locally via Ollama

## Performance Notes

- Messages are fetched in batches of 100 (Telegram API limit)
- AI analysis uses a context window of 20 messages
- Database operations use transactions for efficiency
- A 30-minute scheduler interval balances freshness and API limits

## Troubleshooting

### Authentication Issues
- Delete `telegram.session` and retry
- Verify API credentials in `.env`

### Database Errors
- Ensure libSQL is properly installed and configured
- Check file permissions for the database file

### AI Analysis Issues
- Verify Ollama is running (`curl http://localhost:11434/api/version`)
- Check if the specified model in `.env` exists and is available

### Telegram API Errors

- **PEER_ID_INVALID / CHAT_ID_INVALID / CHANNEL_INVALID**: These errors may occur if the account no longer has access to certain chats or if the chats have been deleted/archived. Review and, if needed, clean up your session or chat list.
- **Rate Limiting**: If errors appear in quick succession, the program automatically waits between requests. You may adjust the scheduler interval if needed.

## Database Architecture

The project employs **libSQL** for its database operations. The database schema includes:

- **chat_messages Table**: Contains chat messages with the following columns:
  - `chat_peer`: Unique identifier for the chat
  - `msg_id`: Message identifier
  - `date`: Timestamp of the message
  - `text`: Message content
  - `urgent`: Flag indicating urgent messages
  - `embedding`: A `F32_BLOB(1024)` storing the vector embedding for the message (computed using the BGE-M3 model via Ollama)

- **Vector Index**: The `libsql_vector_idx(embedding)` index is created on the `embedding` column, enabling efficient similarity searches for future AI functionalities.

- **Data Migration**: A migration script transfers historical data from the legacy SQLite database to the libSQL database and computes embeddings for all messages.

## Contributing

1. Fork the repository
2. Create your feature branch
3. Commit your changes
4. Push to the branch
5. Create a new Pull Request

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Environment

Make sure your `.env` file contains the required configuration for Telegram API credentials and Ollama settings. 