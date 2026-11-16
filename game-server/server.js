const dgram = require('dgram');
const WebSocket = require('ws');
const { DatabaseSync } = require('node:sqlite');
const path = require('path');

const dbPath = path.join(__dirname, '..', 'database', 'dw1_mmo.sqlite');
const db = new DatabaseSync(dbPath);

db.exec(`
CREATE TABLE IF NOT EXISTS Accounts (
    AccountID INTEGER PRIMARY KEY AUTOINCREMENT,
    Username TEXT NOT NULL UNIQUE,
    PasswordHash TEXT NOT NULL,
    CreatedAt TEXT DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS Tamers (
    TamerID INTEGER PRIMARY KEY AUTOINCREMENT,
    AccountID INTEGER NOT NULL REFERENCES Accounts(AccountID),
    TamerName TEXT NOT NULL UNIQUE,
    Bits INTEGER NOT NULL DEFAULT 500,
    ProsperityPoints INTEGER NOT NULL DEFAULT 1,
    MentorPoints INTEGER NOT NULL DEFAULT 0,
    CurrentMapID INTEGER NOT NULL DEFAULT 2,
    PosX INTEGER NOT NULL DEFAULT 0,
    PosY INTEGER NOT NULL DEFAULT 0,
    PosZ INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS ActiveDigimon (
    DigimonInstanceID INTEGER PRIMARY KEY AUTOINCREMENT,
    TamerID INTEGER NOT NULL REFERENCES Tamers(TamerID),
    SpeciesID INTEGER NOT NULL DEFAULT 3,
    Nickname TEXT,
    CurrentHP INTEGER NOT NULL DEFAULT 100,
    MaxHP INTEGER NOT NULL DEFAULT 100,
    CurrentMP INTEGER NOT NULL DEFAULT 100,
    MaxMP INTEGER NOT NULL DEFAULT 100,
    Offense INTEGER NOT NULL DEFAULT 10,
    Defense INTEGER NOT NULL DEFAULT 10,
    Speed INTEGER NOT NULL DEFAULT 10,
    Brains INTEGER NOT NULL DEFAULT 10,
    Discipline INTEGER NOT NULL DEFAULT 50,
    Happiness INTEGER NOT NULL DEFAULT 50,
    CareMistakes INTEGER NOT NULL DEFAULT 0,
    AgeDays INTEGER NOT NULL DEFAULT 0,
    Weight INTEGER NOT NULL DEFAULT 15
);

CREATE TABLE IF NOT EXISTS TamerInventory (
    InventoryID INTEGER PRIMARY KEY AUTOINCREMENT,
    TamerID INTEGER NOT NULL REFERENCES Tamers(TamerID),
    ItemType TEXT NOT NULL,
    ItemID INTEGER NOT NULL,
    Quantity INTEGER NOT NULL DEFAULT 1,
    IsTradeable INTEGER NOT NULL DEFAULT 1,
    UNIQUE(TamerID, ItemType, ItemID)
);

CREATE TABLE IF NOT EXISTS TamerProsperityFlags (
    TamerID INTEGER NOT NULL REFERENCES Tamers(TamerID),
    RecruitedDigimonID INTEGER NOT NULL,
    ProsperityValue INTEGER NOT NULL,
    UnlockedAt TEXT DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (TamerID, RecruitedDigimonID)
);

CREATE TABLE IF NOT EXISTS TamerBossClears (
    TamerID INTEGER NOT NULL REFERENCES Tamers(TamerID),
    BossEventID INTEGER NOT NULL,
    ClearMode TEXT NOT NULL,
    PartySize INTEGER NOT NULL DEFAULT 1,
    ClearedAt TEXT DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (TamerID, BossEventID)
);

INSERT OR IGNORE INTO Accounts (AccountID, Username, PasswordHash) VALUES (1, 'adi_tamer', 'dev_hash');
INSERT OR IGNORE INTO Tamers (TamerID, AccountID, TamerName, Bits, ProsperityPoints) VALUES (1, 1, 'Adi', 500, 1);
INSERT OR IGNORE INTO ActiveDigimon (DigimonInstanceID, TamerID, SpeciesID, Nickname) VALUES (1, 1, 3, 'Agumon');
`);

console.log('[DB] SQLite aktif di:', dbPath);

const onlinePlayers = new Map();
const udpServer = dgram.createSocket('udp4');

udpServer.on('message', (msg, rinfo) => {
    if (msg.length !== 32 || msg.readUInt8(0) !== 0x01) return;

    const mapId          = msg.readUInt8(1);
    const partnerSpecies = msg.readUInt16LE(2);
    const tamerX         = msg.readInt32LE(4);
    const tamerY         = msg.readInt32LE(8);
    const tamerZ         = msg.readInt32LE(12);
    const tamerRotY      = msg.readInt16LE(16);
    const tamerAnim      = msg.readUInt8(18);
    const partnerAnim    = msg.readUInt8(19);
    const partnerX       = msg.readInt32LE(20);
    const partnerZ       = msg.readInt32LE(24);

    const senderKey = `${rinfo.address}:${rinfo.port}`;
    onlinePlayers.set(senderKey, {
        address: rinfo.address, port: rinfo.port,
        mapId, partnerSpecies,
        tamerX, tamerY, tamerZ,
        tamerRotY, tamerAnim, partnerAnim,
        partnerX, partnerZ,
        lastSeen: Date.now()
    });

    for (const [key, peer] of onlinePlayers.entries()) {
        if (key !== senderKey && peer.mapId === mapId) {
            udpServer.send(msg, peer.port, peer.address);
        }
    }
});

udpServer.on('listening', () => {
    const addr = udpServer.address();
    console.log(`[UDP] Fast-Sync Server aktif di ${addr.address}:${addr.port}`);
});

setInterval(() => {
    const now = Date.now();
    for (const [key, p] of onlinePlayers.entries()) {
        if (now - p.lastSeen > 10000) onlinePlayers.delete(key);
    }
}, 5000);

const wss = new WebSocket.Server({ port: 8080 });

wss.on('connection', (ws) => {
    console.log('[WS] Client terhubung');

    ws.on('message', (raw) => {
        try {
            const data = JSON.parse(raw);
            if (data.type === 'LOGIN') {
                const tamer = db.prepare('SELECT * FROM Tamers WHERE TamerID = ?').get(data.tamerId || 1);
                const digimon = db.prepare('SELECT * FROM ActiveDigimon WHERE TamerID = ?').get(data.tamerId || 1);
                const flags = db.prepare('SELECT RecruitedDigimonID, ProsperityValue FROM TamerProsperityFlags WHERE TamerID = ?').all(data.tamerId || 1);

                ws.send(JSON.stringify({
                    type: 'LOGIN_SUCCESS',
                    tamer,
                    digimon,
                    worldPhasingFlags: flags
                }));
            }
        } catch (err) {
            console.error('[WS] Error:', err.message);
        }
    });
});

console.log('[WS] Authoritative Server aktif di port 8080');
udpServer.bind(7777);