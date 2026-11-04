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
- **Rotate PDF**: Rotate pages vertically or horizontally (90°, 180°, 270° clockwise).
- **Organize PDF**: Rearrange pages visually with drag-and-drop thumbnails or arrow buttons.
- **Secure File Handling**: Leverages `tempfile` for safe temporary storage with automatic cleanup after processing.
---

## 🛠️ Tech Stack

- **Frontend**: HTML5, Vanilla JavaScript, Tailwind CSS (compiled locally with the Tailwind CLI)
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

### Rebuilding the CSS

The compiled stylesheet (`static/output.css`) is committed, so `cargo run` works out of the box.
If you change `static/index.html`, `static/input.css` or `tailwind.config.js`, regenerate it:

```bash
npm install
npm run build:css   # one-off build
npm run watch:css   # rebuild on change while developing
```

---

## 🔌 API Endpoints

| Method | Endpoint | Description |
| :--- | :--- | :--- |
| `GET` | `/` | Serves the web interface (`index.html`) |
| `POST` | `/api/compress` | Uploads a PDF file and returns the compressed version |
| `POST` | `/api/split` | Uploads a PDF file and splits/extracts pages |
| `POST` | `/api/merge` | Uploads multiple PDF files and merges them into one |
| `POST` | `/api/rotate` | Rotates pages by specified degrees (90°, 180°, 270°) |
| `POST` | `/api/organize` | Rearranges pages according to the `order` field (1-based page numbers) |
| `POST` | `/api/pages-info` | Returns the page count of an uploaded PDF as JSON (`{"page_count": N}`) |

---

## 🛡️ License

Distributed under the MIT License. See `LICENSE` for more information.
