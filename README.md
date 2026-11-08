# ihatepdf 📄⚡

> A lightning-fast, lightweight web application that mimics iLovePDF — built with **Rust (Axum + Tokio)** on the backend and **vanilla JavaScript + Tailwind CSS** on the frontend.

![License](https://img.shields.io/badge/license-MIT-red.svg)
![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)
![Axum](https://img.shields.io/badge/axum-0.7-blue.svg)
![Tailwind CSS](https://img.shields.io/badge/tailwind-v3-38bdf8.svg)

---

## ✨ Features

- **Compress PDF**: Shrinks files by removing invisible application-private data (`/PieceInfo`) and recompressing streams. It never returns a file larger than the original.
- **Split PDF**: Pick pages from an interactive thumbnail grid and extract them into a new PDF.
- **Merge PDF**: Combine multiple PDF files into a single document, preserving file and page order.
- **Rotate PDF**: Rotate pages vertically or horizontally (90°, 180°, 270° clockwise).
- **Organize PDF**: Rearrange pages visually with drag-and-drop thumbnails or arrow buttons.
- **Secure File Handling**: Uploads are processed entirely in memory and never written to disk.
---

## 🛠️ Tech Stack

- **Frontend**: HTML5, Vanilla JavaScript, Tailwind CSS (compiled locally with the Tailwind CLI), inline SVG icons
- **Backend**: Rust, Axum, Tokio, Tower HTTP, Lopdf

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
| `POST` | `/api/compress` | Compresses a PDF (drops application-private `/PieceInfo` data, recompresses streams) and never returns a bigger file |
| `POST` | `/api/split` | Extracts the pages listed in the optional `pages` field (`"1-3,5"`, 1-based) into a new PDF |
| `POST` | `/api/merge` | Uploads multiple PDF files (field `files`) and merges them in order |
| `POST` | `/api/rotate` | Rotates pages by specified degrees (90°, 180°, 270°) |
| `POST` | `/api/organize` | Rearranges pages according to the `order` field (1-based page numbers) |
| `POST` | `/api/pages-info` | Returns the page count of an uploaded PDF as JSON (`{"page_count": N}`) |

> Uploads are capped at **50 MB** per request. Password-protected PDFs are rejected with a clear error message.

---

## 🛡️ License

Distributed under the MIT License. See `LICENSE` for more information.
