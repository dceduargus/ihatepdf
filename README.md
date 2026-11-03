# ihatepdf 📄⚡

> A lightning-fast, lightweight web application that mimics iLovePDF — built with **Rust (Axum + Tokio)** on the backend and **vanilla JavaScript + Tailwind CSS** on the frontend.

![License](https://img.shields.io/badge/license-MIT-red.svg)
![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)
![Axum](https://img.shields.io/badge/axum-0.7-blue.svg)
![Tailwind CSS](https://img.shields.io/badge/tailwind-v3-38bdf8.svg)

---

## ✨ Features

- **Compress PDF**: Reduce file sizes while retaining optimal document quality.
- **Split PDF**: Extract pages and split documents effortlessly.
- **Merge PDF**: Combine multiple PDF files into a single unified document.
- **Lightning Fast**: Powered by Rust and Tokio async runtime for maximum throughput and minimal resource usage.
- **Secure File Handling**: Leverages `tempfile` for safe temporary storage with automatic cleanup after processing.

---

## 🛠️ Tech Stack

- **Frontend**: HTML5, Vanilla JavaScript, Tailwind CSS (via CDN)
- **Backend**: Rust, Axum, Tokio, Tower HTTP, Multer, Lopdf, Tempfile

---

## 🚀 Getting Started

### Prerequisites

Make sure you have Rust and Cargo installed:
- [Install Rust](https://www.rust-lang.org/tools/install)

### Running Locally

1. **Clone the repository**:
   ```bash
   git clone https://github.com/your-username/ihatepdf.git
   cd ihatepdf
   ```

2. **Run the server**:
   ```bash
   cargo run
   ```

3. **Open in your browser**:
   Navigate to [http://127.0.0.1:3000](http://127.0.0.1:3000)

---

## 🔌 API Endpoints

| Method | Endpoint | Description |
| :--- | :--- | :--- |
| `GET` | `/` | Serves the web interface (`index.html`) |
| `POST` | `/api/compress` | Uploads a PDF file and returns the compressed version |
| `POST` | `/api/split` | Uploads a PDF file and extracts/splits pages |
| `POST` | `/api/merge` | Uploads multiple PDF files and merges them into one |

---

## 🛡️ License

Distributed under the MIT License. See `LICENSE` for more information.
