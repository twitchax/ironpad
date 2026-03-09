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
    const DB_VERSION = 1;
    const STORE_NAME = 'notebooks';

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
            };
            req.onsuccess = (e) => resolve(e.target.result);
            req.onerror = (e) => reject(e.target.error);
        });
    }

    function tx(db, mode) {
        return db.transaction(STORE_NAME, mode).objectStore(STORE_NAME);
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
            const store = tx(db, 'readonly');
            const all = await reqToPromise(store.getAll());
            db.close();
            // Sort by updated_at descending (most recent first).
            all.sort((a, b) => {
                const ta = new Date(a.updated_at).getTime();
                const tb = new Date(b.updated_at).getTime();
                return tb - ta;
            });
            return all;
        },

        /**
         * Get a single notebook by ID.
         * @param {string} id - Notebook UUID.
         * @returns {Promise<Object|undefined>} The notebook, or undefined if not found.
         */
        getNotebook: async function (id) {
            const db = await openDb();
            const store = tx(db, 'readonly');
            const result = await reqToPromise(store.get(id));
            db.close();
            return result || null;
        },

        /**
         * Save (upsert) a notebook. Sets updated_at to now.
         * @param {Object} notebook - IronpadNotebook object. Must have an `id` field.
         * @returns {Promise<void>}
         */
        saveNotebook: async function (notebook) {
            notebook.updated_at = new Date().toISOString();
            const db = await openDb();
            const store = tx(db, 'readwrite');
            await reqToPromise(store.put(notebook));
            db.close();
        },

        /**
         * Delete a notebook by ID.
         * @param {string} id - Notebook UUID.
         * @returns {Promise<void>}
         */
        deleteNotebook: async function (id) {
            const db = await openDb();
            const store = tx(db, 'readwrite');
            await reqToPromise(store.delete(id));
            db.close();
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
            return all.filter((nb) => nb.title.toLowerCase().includes(q));
        },

        /**
         * Export a notebook as a JSON string.
         * @param {string} id - Notebook UUID.
         * @returns {Promise<string|null>} JSON string, or null if not found.
         */
        exportNotebook: async function (id) {
            const nb = await this.getNotebook(id);
            return nb ? JSON.stringify(nb, null, 2) : null;
        },

        /**
         * Import a notebook from a JSON string. Assigns a new UUID so
         * the imported copy doesn't collide with the original.
         * @param {string} jsonString - JSON-encoded IronpadNotebook.
         * @returns {Promise<Object>} The imported notebook (with new ID).
         */
        importNotebook: async function (jsonString) {
            const nb = JSON.parse(jsonString);
            nb.id = crypto.randomUUID();
            nb.created_at = new Date().toISOString();
            nb.updated_at = new Date().toISOString();
            await this.saveNotebook(nb);
            return nb;
        },
    };
})();
