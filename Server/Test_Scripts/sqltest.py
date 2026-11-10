import sqlite3
c = sqlite3.connect(':memory:')
c.executescript('CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL, from_name TEXT NOT NULL, text TEXT NOT NULL, timestamp INTEGER NOT NULL);')
c.execute("INSERT INTO messages (channel_id, from_name, text, timestamp) VALUES (1, 'a', 'x', 1)")
print('insert ok')
try:
    c.execute('DELETE FROM messages WHERE channel_id = 1 AND id NOT IN (SELECT id FROM messages WHERE channel_id = 1 ORDER BY id DESC LIMIT 100)')
    print('retention delete ok')
except Exception as e:
    print('RETENTION FEHLER:', e)
try:
    rows = c.execute('SELECT channel_id, from_name, text, timestamp FROM (SELECT channel_id, from_name, text, timestamp, ROW_NUMBER() OVER (PARTITION BY channel_id ORDER BY id DESC) AS rn FROM messages) WHERE rn <= 100 ORDER BY channel_id, timestamp, rowid').fetchall()
    print('window query ok:', rows)
except Exception as e:
    print('WINDOW FEHLER:', e)
