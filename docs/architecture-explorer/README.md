# Architecture explorer

Open [`index.html`](index.html) directly in a browser. This single file works via `file://`, can be copied elsewhere, and includes all styles, scripts, content, and source excerpts. Viewing requires no server, build tools, network connection, account, or API. JavaScript must be enabled. Optional GitHub links need a network connection.

The explorer is an educational simulation of the `main` baseline at **`8cb41624eb2dc2ca87e4409961f42553ceb029cf`**. It does not execute requests, inspect local configuration, or track the current checkout. Responsibility groups describe module ownership; they are not a deployment or process diagram.

Use the architecture map to inspect ownership and distinguish runtime calls, data flow, and code dependencies. The request walkthrough follows one representative path. The four boundary scenarios offer selected outcomes, with durable facts and client visibility shown at each step. Deep dives explain additional ownership boundaries. These curated paths explain the cited implementation; they do not enumerate every runtime branch. Source disclosures remain available offline in each mode.

Rebuild from the repository root with Node.js and Git:

```bash
node docs/architecture-explorer/build.mjs
node docs/architecture-explorer/build.mjs --check
```

No package installation is needed. The baseline commit must be available in the local Git object database. `build.mjs` reads `content.json`, extracts source lines from that exact commit with `git show`, and embeds them alongside `styles.css` and `app.js` in `index.template.html`. It never substitutes `HEAD` for the baseline. Generated links also point to that exact commit.

`--check` validates the content and compares the expected output with the committed `index.html` without writing files. The build rejects invalid IDs and references, unknown relationship kinds, unsafe source paths, invalid line bounds, and excerpts longer than 80 lines. Output is deterministic and includes no timestamps or environment-specific paths. JSON is escaped for safe embedding in an inline script element.

Edit `content.json` for explanations and evidence, `app.js` for interaction, `styles.css` for presentation, and `index.template.html` for the document shell. Rebuild and commit the generated `index.html` with its sources. To intentionally change the documented baseline, update the explicit baseline in both `content.json` and `build.mjs`, review every explanation and source range against the new snapshot, and update this document.
