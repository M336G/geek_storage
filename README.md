# GEEK Storage
**Superfast caching server for [GEEK Music](https://top.gg/bot/971868710237274174). Rust full-rewrite of my [storage-server](https://github.com/M336G/storage-server)**

## Prerequisites
- [Rust](https://rust-lang.org/) installed
- A device or server capable of running 24/7

## Running
**1.** [Download the repository manually](https://github.com/M336G/geek_storage/archive/refs/heads/main.zip) or clone it:
```bash
git clone https://github.com/M336G/geek_storage.git
cd geek_storage
```

**2.** Create a `.env` file and fill it according to your needs using [`.env.example`](https://github.com/M336G/geek_storage/blob/main/.env.example) as a template

**3.** Start the instance with:
- `cargo run --release` for production
- `cargo run` for development/testing

**Once you've done all of this, you should have a running instance!**

*You may also download a pre-built binary directly from the [releases tab](https://github.com/M336G/geek_storage/releases/latest).*

## Usage
Once you've got your instance running, you may use these endpoints:

| Method | Endpoint | Description                         |
|--------|----------|-------------------------------------|
| `GET`  | `/`      | Check if the server's up or not     |
| `POST` | `/`      | Upload a file (or one from a link)  |
| `GET`  | `/<id>`  | Download an existing file           |
| `GET`  | `/info`  | Get information about the server    |

## License
This project is licensed under the [MIT License](https://github.com/M336G/geek_storage/blob/main/LICENSE).