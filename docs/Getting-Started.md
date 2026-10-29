# Getting Started

Let's get from zero to a query result. We'll use SQLite, because it needs nothing but a file.

## Install

Grab a build from [Releases](https://github.com/thekodeking/quarry/releases): a `.dmg` for macOS,
`.deb`, `.AppImage`, or `.rpm` for Linux (x86_64 or ARM64), or an installer for Windows. On Fedora, install the matching RPM with:

```sh
sudo dnf install ./quarry-*.rpm
```

On macOS the app is ad-hoc signed, not notarized, so Gatekeeper will refuse it the first time.
Tell it you mean it:

```sh
xattr -dr com.apple.quarantine /Applications/Quarry.app
```

Or build it yourself, and macOS won't complain at all:

```sh
git clone https://github.com/thekodeking/quarry && cd quarry
make run
```

## Something to look at

Make a tiny shop:

```sh
sqlite3 shop.db <<'SQL'
CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT, email TEXT);
CREATE TABLE orders (
  id INTEGER PRIMARY KEY,
  customer_id INTEGER REFERENCES customers(id),
  total REAL
);
INSERT INTO customers VALUES (1, 'Ada', 'ada@example.com'), (2, 'Grace', 'grace@exmaple.com');
INSERT INTO orders VALUES (1, 1, 42.50), (2, 1, 19.00), (3, 2, 99.95);
SQL
```

Yes, Grace's email has a typo. We'll fix it in [Editing Data Safely](Editing-Data-Safely).

## Connect

Press `⇧⌘N` (`Ctrl+Shift+N` elsewhere) for a new connection, pick SQLite, and point it at `shop.db`.

You don't have to fill in fields one by one. Paste a URL and Quarry figures out the driver:

```
postgres://me@localhost:5432/shop
mysql://root@127.0.0.1:3306/shop
sqlserver://sa@127.0.0.1:1433/master
oracle://scott@db.internal:1521/FREEPDB1
```

Passwords go into your OS keychain, not into a config file. More on that in
[Connections and Production](Connections-and-Production).

## Run something

`⌘T` opens a query tab. Type:

```sql
SELECT c.name, sum(o.total) AS spent
FROM customers c
JOIN orders o ON o.customer_id = c.id
GROUP BY c.name;
```

Press `⌘↵`. Quarry runs *the statement under the cursor*, not the whole file, so you can keep a
scratchpad of twenty queries and run them one at a time. `⇧⌘↵` runs them all.

Now select both cells in the `spent` column. The status bar shows the sum, average, min and max.
No `SELECT sum(...)` required.

## Three keys to remember

| Key | Does |
|---|---|
| `⌘K` | The command bar. Tables, recent queries, every action. When in doubt, press this. |
| `⌘↵` | Run the statement at the cursor. |
| `⌘S` | Review staged edits before they hit the database. |

Everything else you can find through `⌘K`.
