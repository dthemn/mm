# Third-party recognizer files

These files are downloaded at build time by `web/ocr-assets.sh` and checked against pinned hashes.

- PP-OCRv5 mobile detection and Latin recognition models, from PaddleOCR (Apache License 2.0),
  in the ONNX form published by RapidOCR (Apache License 2.0).
- `latin_dict.txt`, the character list stored inside the recognition model.
- ONNX Runtime Web 1.31.1 (MIT License).
