-- DW1 MMO schema (SQLite). Dijalankan otomatis oleh game-server saat start.
-- Semua statement idempotent (IF NOT EXISTS).

CREATE TABLE IF NOT EXISTS Accounts (
    AccountID    INTEGER PRIMARY KEY AUTOINCREMENT,
    Username     TEXT NOT NULL UNIQUE COLLATE NOCASE,
    PasswordHash TEXT NOT NULL,
    CreatedAt    TEXT DEFAULT CURRENT_TIMESTAMP,
    LastLogin    TEXT
);

CREATE TABLE IF NOT EXISTS Tamers (
    TamerID          INTEGER PRIMARY KEY AUTOINCREMENT,
    AccountID        INTEGER NOT NULL REFERENCES Accounts(AccountID),
    TamerName        TEXT NOT NULL UNIQUE COLLATE NOCASE,
    Bits             INTEGER NOT NULL DEFAULT 500,
    ProsperityPoints INTEGER NOT NULL DEFAULT 1,
    MentorPoints     INTEGER NOT NULL DEFAULT 0,
    CurrentMapID     INTEGER NOT NULL DEFAULT 2,
    PosX             INTEGER NOT NULL DEFAULT 0,
    PosY             INTEGER NOT NULL DEFAULT 0,
    PosZ             INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS IX_Tamers_Account ON Tamers(AccountID);

CREATE TABLE IF NOT EXISTS ActiveDigimon (
    DigimonInstanceID INTEGER PRIMARY KEY AUTOINCREMENT,
    TamerID      INTEGER NOT NULL REFERENCES Tamers(TamerID),
    SpeciesID    INTEGER NOT NULL DEFAULT 3,
    Nickname     TEXT,
    CurrentHP    INTEGER NOT NULL DEFAULT 100,
    MaxHP        INTEGER NOT NULL DEFAULT 100,
    CurrentMP    INTEGER NOT NULL DEFAULT 100,
    MaxMP        INTEGER NOT NULL DEFAULT 100,
    Offense      INTEGER NOT NULL DEFAULT 10,
    Defense      INTEGER NOT NULL DEFAULT 10,
    Speed        INTEGER NOT NULL DEFAULT 10,
    Brains       INTEGER NOT NULL DEFAULT 10,
    Discipline   INTEGER NOT NULL DEFAULT 50,
    Happiness    INTEGER NOT NULL DEFAULT 50,
    CareMistakes INTEGER NOT NULL DEFAULT 0,
    AgeDays      INTEGER NOT NULL DEFAULT 0,
    Weight       INTEGER NOT NULL DEFAULT 15,
    UpdatedAt    TEXT
);
CREATE INDEX IF NOT EXISTS IX_ActiveDigimon_Tamer ON ActiveDigimon(TamerID);

CREATE TABLE IF NOT EXISTS TamerInventory (
    InventoryID INTEGER PRIMARY KEY AUTOINCREMENT,
    TamerID     INTEGER NOT NULL REFERENCES Tamers(TamerID),
    ItemType    TEXT NOT NULL,
    ItemID      INTEGER NOT NULL,
    Quantity    INTEGER NOT NULL DEFAULT 1,
    IsTradeable INTEGER NOT NULL DEFAULT 1,
    UNIQUE(TamerID, ItemType, ItemID)
);

CREATE TABLE IF NOT EXISTS TamerProsperityFlags (
    TamerID            INTEGER NOT NULL REFERENCES Tamers(TamerID),
    RecruitedDigimonID INTEGER NOT NULL,
    ProsperityValue    INTEGER NOT NULL,
    UnlockedAt         TEXT DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (TamerID, RecruitedDigimonID)
);

CREATE TABLE IF NOT EXISTS TamerBossClears (
    TamerID     INTEGER NOT NULL REFERENCES Tamers(TamerID),
    BossEventID INTEGER NOT NULL,
    ClearMode   TEXT NOT NULL,
    PartySize   INTEGER NOT NULL DEFAULT 1,
    ClearedAt   TEXT DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (TamerID, BossEventID)
);
