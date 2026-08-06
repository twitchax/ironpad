/**
 * IronpadStorage — IndexedDB storage layer for private notebooks.
 *
 * All methods are async and work with IronpadNotebook JSON objects matching:
 * {
 *   version: 1,
 *   id: "uuid-string",
 *   title: "...",
 *   created_at: "ISO 8601",
 *   updated_at: "ISO 8601",
 *   shared_cargo_toml: "..." | null,
 *   cells: [{ id, order, label, cell_type, source, cargo_toml }]
 * }
 */
window.IronpadStorage = (function () {
    const DB_NAME = 'ironpad';
    const DB_VERSION = 6;
    const STORE_NAME = 'notebooks';
    // Local compiled-WASM blob cache (PRD-0047), content-addressed by the
    // same cache key the server uses. LRU-pruned to MAX_BLOB_ENTRIES.
    const BLOB_STORE = 'blobs';
    const MAX_BLOB_ENTRIES = 64;
    // Retired stores, deleted by migration: 'mutable' held PRD-0049/0053
    // working copies of published notebooks — those live entirely on the
    // server now (PRD-0054).
    const RETIRED_MUTABLE_STORE = 'mutable';
    // Device-scoped metadata, keyed by 'name'. Held over from PRD-0049 (it
    // stored the user key); kept as an empty store for future device prefs.
    const META_STORE = 'meta';
    // Version history for private notebooks (PRD-0058): a rolling snapshot
    // ring, one snapshot per bucket per notebook, pruned to a cap, deleted
    // with the notebook. Local-only insurance — never synced anywhere.
    const HISTORY_STORE = 'history';
    const HISTORY_BUCKET_MS = 5 * 60 * 1000;
    const HISTORY_CAP = 30;

    function openDb() {
        return new Promise((resolve, reject) => {
            const req = indexedDB.open(DB_NAME, DB_VERSION);
            req.onupgradeneeded = (e) => {
                const db = e.target.result;
                if (!db.objectStoreNames.contains(STORE_NAME)) {
                    const store = db.createObjectStore(STORE_NAME, { keyPath: 'id' });
                    store.createIndex('updated_at', 'updated_at');
                    store.createIndex('title', 'title');
                }
                if (!db.objectStoreNames.contains(BLOB_STORE)) {
                    const blobs = db.createObjectStore(BLOB_STORE, { keyPath: 'hash' });
                    blobs.createIndex('lastUsed', 'lastUsed');
                }
                if (!db.objectStoreNames.contains(META_STORE)) {
                    db.createObjectStore(META_STORE, { keyPath: 'name' });
                }
                // v6 (PRD-0058): the version-history ring.
                if (!db.objectStoreNames.contains(HISTORY_STORE)) {
                    const history = db.createObjectStore(HISTORY_STORE, {
                        keyPath: ['notebookId', 'savedAt'],
                    });
                    history.createIndex('notebookId', 'notebookId');
                }
                // v5 (PRD-0054): published notebooks live on the server, so
                // the local working-copy store dies. Any 0.15.0-era working
                // copies are discarded (scorched earth; nothing real existed).
                // Also covers the v4 cleanup for skipped versions.
                if (db.objectStoreNames.contains(RETIRED_MUTABLE_STORE)) {
                    db.deleteObjectStore(RETIRED_MUTABLE_STORE);
                }
                if (e.oldVersion > 0 && e.oldVersion < 5) {
                    e.target.transaction.objectStore(META_STORE).delete('user_key');
                }
            };
            req.onsuccess = (e) => resolve(e.target.result);
            req.onerror = (e) => reject(e.target.error);
            req.onblocked = () => reject(new Error('IndexedDB open blocked'));
        });
    }

    function tx(db, mode) {
        return db.transaction(STORE_NAME, mode).objectStore(STORE_NAME);
    }

    function historyTx(db, mode) {
        return db.transaction(HISTORY_STORE, mode).objectStore(HISTORY_STORE);
    }

    /** All history entries for a notebook, newest first. */
    async function historyEntries(db, notebookId) {
        const idx = historyTx(db, 'readonly').index('notebookId');
        const entries = await reqToPromise(idx.getAll(notebookId));
        entries.sort((a, b) => b.savedAt - a.savedAt);
        return entries;
    }

    /**
     * Write a snapshot of `nb` into the history ring. Unless `force`, the
     * write is skipped when the notebook's newest snapshot is younger than
     * the bucket (one snapshot per HISTORY_BUCKET_MS per notebook); every
     * write prunes the ring to HISTORY_CAP.
     */
    async function writeHistorySnapshot(db, nb, force) {
        const entries = await historyEntries(db, nb.id);
        const now = Date.now();
        if (!force && entries.length > 0 && now - entries[0].savedAt < HISTORY_BUCKET_MS) {
            return;
        }
        const store = historyTx(db, 'readwrite');
        await reqToPromise(store.put({
            notebookId: nb.id,
            // Nudge past an existing key on a same-millisecond force write.
            savedAt: entries.length > 0 && entries[0].savedAt >= now ? entries[0].savedAt + 1 : now,
            json: JSON.stringify(nb),
        }));
        for (const stale of entries.slice(HISTORY_CAP - 1)) {
            await reqToPromise(store.delete([stale.notebookId, stale.savedAt]));
        }
    }

    function blobTx(db, mode) {
        return db.transaction(BLOB_STORE, mode).objectStore(BLOB_STORE);
    }

    function reqToPromise(request) {
        return new Promise((resolve, reject) => {
            request.onsuccess = () => resolve(request.result);
            request.onerror = () => reject(request.error);
        });
    }

    return {
        /**
         * List all notebooks, sorted by updated_at descending.
         * @returns {Promise<Array>} Array of IronpadNotebook objects.
         */
        listNotebooks: async function () {
            const db = await openDb();
            try {
                const store = tx(db, 'readonly');
                const all = await reqToPromise(store.getAll());
                // Sort by updated_at descending (most recent first). Malformed
                // updated_at values yield NaN from getTime(); coerce to 0 so
                // they sort last instead of breaking comparator transitivity.
                all.sort((a, b) => {
                    const ta = new Date(a.updated_at).getTime() || 0;
                    const tb = new Date(b.updated_at).getTime() || 0;
                    return tb - ta;
                });
                return all;
            } finally {
                db.close();
            }
        },

        /**
         * Get a single notebook by ID.
         * @param {string} id - Notebook UUID.
         * @returns {Promise<Object|undefined>} The notebook, or undefined if not found.
         */
        getNotebook: async function (id) {
            const db = await openDb();
            try {
                const result = await reqToPromise(tx(db, 'readonly').get(id));
                return result || null;
            } finally {
                db.close();
            }
        },

        /**
         * Save (upsert) a notebook to the private store. Sets updated_at to
         * now. (Published notebooks persist server-side — PRD-0054 — and
         * never pass through here.)
         * @param {Object} notebook - IronpadNotebook object. Must have an `id` field.
         * @returns {Promise<void>}
         */
        saveNotebook: async function (notebook) {
            // Clone rather than mutate the caller's object.
            const nb = { ...notebook, updated_at: new Date().toISOString() };
            const db = await openDb();
            try {
                await reqToPromise(tx(db, 'readwrite').put(nb));
                // History capture (PRD-0058) rides the save choke point;
                // best-effort — a history failure must never fail the save.
                try {
                    await writeHistorySnapshot(db, nb, false);
                } catch (e) {
                    console.warn('ironpad: history snapshot failed:', e);
                }
            } finally {
                db.close();
            }
        },

        /**
         * Delete a notebook by ID.
         * @param {string} id - Notebook UUID.
         * @returns {Promise<void>}
         */
        deleteNotebook: async function (id) {
            const db = await openDb();
            try {
                const store = tx(db, 'readwrite');
                await reqToPromise(store.delete(id));
                // The notebook's history dies with it (PRD-0058).
                const entries = await historyEntries(db, id);
                const history = historyTx(db, 'readwrite');
                for (const entry of entries) {
                    await reqToPromise(history.delete([entry.notebookId, entry.savedAt]));
                }
            } finally {
                db.close();
            }
        },

        /**
         * Search notebooks by title (case-insensitive substring match).
         * @param {string} query - Search string.
         * @returns {Promise<Array>} Matching notebooks sorted by updated_at desc.
         */
        searchNotebooks: async function (query) {
            const all = await this.listNotebooks();
            if (!query || !query.trim()) return all;
            const q = query.toLowerCase().trim();
            // A stored notebook may lack a title (hand-written or a partial
            // import): treat a missing/non-string title as a non-match rather
            // than throwing on `.toLowerCase()`.
            return all.filter(
                (nb) => typeof nb.title === "string" && nb.title.toLowerCase().includes(q)
            );
        },

        /**
         * List a notebook's history snapshots, newest first (PRD-0058).
         * Meta only — the JSON stays in the store until a restore asks.
         * @param {string} id - Notebook UUID.
         * @returns {Promise<Array<{savedAt: number, title: string, cellCount: number}>>}
         */
        listHistory: async function (id) {
            const db = await openDb();
            try {
                const entries = await historyEntries(db, id);
                return entries.map((entry) => {
                    let title = '';
                    let cellCount = 0;
                    try {
                        const nb = JSON.parse(entry.json);
                        title = typeof nb.title === 'string' ? nb.title : '';
                        cellCount = Array.isArray(nb.cells) ? nb.cells.length : 0;
                    } catch (e) { /* corrupt snapshot: listed, restore will fail loudly */ }
                    return { savedAt: entry.savedAt, title: title, cellCount: cellCount };
                });
            } finally {
                db.close();
            }
        },

        /**
         * Fetch one history snapshot's JSON (PRD-0058).
         * @returns {Promise<string|null>}
         */
        getHistorySnapshot: async function (id, savedAt) {
            const db = await openDb();
            try {
                const entry = await reqToPromise(
                    historyTx(db, 'readonly').get([id, savedAt])
                );
                return entry ? entry.json : null;
            } finally {
                db.close();
            }
        },

        /**
         * Force-snapshot the CURRENT stored record, bypassing the bucket —
         * the undoable-restore half of PRD-0058: restore calls this first so
         * the pre-restore state lands in history.
         * @returns {Promise<void>}
         */
        snapshotNow: async function (id) {
            const db = await openDb();
            try {
                const nb = await reqToPromise(tx(db, 'readonly').get(id));
                if (nb) {
                    await writeHistorySnapshot(db, nb, true);
                }
            } finally {
                db.close();
            }
        },

        /**
         * Import a notebook from a JSON string. Assigns a new UUID so
         * the imported copy doesn't collide with the original.
         * @param {string} jsonString - JSON-encoded IronpadNotebook.
         * @returns {Promise<Object>} The imported notebook (with new ID).
         */
        importNotebook: async function (jsonString) {
            // Clone (via spread) rather than mutate the parsed object in place.
            const nb = {
                ...JSON.parse(jsonString),
                id: crypto.randomUUID(),
                created_at: new Date().toISOString(),
                updated_at: new Date().toISOString(),
            };
            await this.saveNotebook(nb);
            return nb;
        },

        // ── Compiled-blob cache (PRD-0047) ─────────────────────────────

        /**
         * Look up a locally cached compiled blob by content hash, touching
         * its LRU timestamp on hit.
         * @param {string} hash - 64-hex cache key.
         * @returns {Promise<Object|null>} { hash, wasm: Uint8Array, glue,
         *          diagnostics, lastUsed } or null.
         */
        getBlob: async function (hash) {
            const db = await openDb();
            try {
                const store = blobTx(db, 'readwrite');
                const record = await reqToPromise(store.get(hash));
                if (!record) return null;
                record.lastUsed = Date.now();
                await reqToPromise(store.put(record));
                return record;
            } finally {
                db.close();
            }
        },

        /**
         * Store (upsert) a compiled blob under its content hash, then prune
         * the least-recently-used entries past MAX_BLOB_ENTRIES. Pruning
         * walks a keys-only cursor on the lastUsed index so it never loads
         * blob bytes.
         * @param {string} hash - 64-hex cache key.
         * @param {Uint8Array} wasm - Compiled WASM bytes.
         * @param {string|null} glue - wasm-bindgen JS glue module, if any.
         * @param {string|null} diagnostics - JSON-encoded diagnostics array.
         * @param {number} preambleLines - Scaffold preamble offset the stored
         *        diagnostics still need (mirrors CompileResponse).
         * @returns {Promise<void>}
         */
        putBlob: async function (hash, wasm, glue, diagnostics, preambleLines) {
            const db = await openDb();
            try {
                const store = blobTx(db, 'readwrite');
                await reqToPromise(store.put({
                    hash,
                    wasm,
                    glue: glue ?? null,
                    diagnostics: diagnostics ?? null,
                    preambleLines: preambleLines ?? 0,
                    lastUsed: Date.now(),
                }));

                const count = await reqToPromise(store.count());
                let excess = count - MAX_BLOB_ENTRIES;
                if (excess > 0) {
                    // Oldest-first over the lastUsed index; primary keys only.
                    const cursorReq = store.index('lastUsed').openKeyCursor();
                    await new Promise((resolve, reject) => {
                        cursorReq.onsuccess = () => {
                            const cursor = cursorReq.result;
                            if (!cursor || excess <= 0) {
                                resolve();
                                return;
                            }
                            store.delete(cursor.primaryKey);
                            excess -= 1;
                            cursor.continue();
                        };
                        cursorReq.onerror = () => reject(cursorReq.error);
                    });
                }
            } finally {
                db.close();
            }
        },
    };
})();
