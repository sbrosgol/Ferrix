# Security Policy

Ferrix opens untrusted documents, so we treat crashes and memory errors triggered by a PDF as
security issues.

## Reporting a vulnerability

Please **do not** open a public issue. Use GitHub's private vulnerability reporting
("Report a vulnerability" under this repository's **Security** tab). Include a sample file or
reproduction steps if you can.

We aim to acknowledge reports within 3 working days and to agree on a disclosure timeline with you.

## Scope

- Crashes, hangs or excessive memory use caused by a document.
- Any way a document can run code, read or write files, or make network requests without the
  user's explicit action.
- Vulnerabilities in the bundled PDFium build are in scope; we update the pinned version promptly.
