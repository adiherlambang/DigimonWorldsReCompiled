'use strict';
const fs = require('fs');
const { DatabaseSync } = require('node:sqlite');

function hasColumn(db, table, col) {
  return db.prepare(`PRAGMA table_info(${table})`).all().some((c) => c.name === col);
}

function openDb(dbPath, schemaPath) {
  const db = new DatabaseSync(dbPath);
  db.exec('PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 3000;');
  db.exec(fs.readFileSync(schemaPath, 'utf8'));
  // Migrasi ringan untuk DB lama yang dibuat server versi prototipe.
  if (!hasColumn(db, 'Accounts', 'LastLogin')) db.exec('ALTER TABLE Accounts ADD COLUMN LastLogin TEXT');
  if (!hasColumn(db, 'ActiveDigimon', 'UpdatedAt')) db.exec('ALTER TABLE ActiveDigimon ADD COLUMN UpdatedAt TEXT');
  return db;
}

class DuplicateError extends Error {
  constructor(field) { super(`duplicate ${field}`); this.field = field; }
}

function createStore(db) {
  const q = {
    accountByName: db.prepare('SELECT AccountID, Username, PasswordHash FROM Accounts WHERE Username = ?'),
    insertAccount: db.prepare('INSERT INTO Accounts (Username, PasswordHash) VALUES (?, ?)'),
    insertTamer: db.prepare('INSERT INTO Tamers (AccountID, TamerName) VALUES (?, ?)'),
    insertDigimon: db.prepare("INSERT INTO ActiveDigimon (TamerID, SpeciesID, Nickname, UpdatedAt) VALUES (?, 3, 'Agumon', CURRENT_TIMESTAMP)"),
    touchLogin: db.prepare('UPDATE Accounts SET LastLogin = CURRENT_TIMESTAMP WHERE AccountID = ?'),
    tamerByAccount: db.prepare(
      'SELECT TamerID, TamerName, Bits, ProsperityPoints, MentorPoints, CurrentMapID, PosX, PosY, PosZ FROM Tamers WHERE AccountID = ? ORDER BY TamerID LIMIT 1'),
    digimon: db.prepare('SELECT * FROM ActiveDigimon WHERE TamerID = ? ORDER BY DigimonInstanceID LIMIT 1'),
    flags: db.prepare('SELECT RecruitedDigimonID, ProsperityValue FROM TamerProsperityFlags WHERE TamerID = ?'),
    bossClears: db.prepare('SELECT BossEventID, ClearMode, PartySize FROM TamerBossClears WHERE TamerID = ?'),
    inventory: db.prepare('SELECT ItemType, ItemID, Quantity, IsTradeable FROM TamerInventory WHERE TamerID = ?'),
    savePos: db.prepare('UPDATE Tamers SET CurrentMapID = ?, PosX = ?, PosY = ?, PosZ = ? WHERE TamerID = ?'),
  };

  function dupField(err) {
    const m = /UNIQUE constraint failed: (\w+)\.(\w+)/.exec(String(err && err.message));
    if (!m) return null;
    return m[1] === 'Accounts' ? 'username' : 'tamerName';
  }

  return {
    getAccount: (username) => q.accountByName.get(username),

    // Account + Tamer + Digimon partner awal dibuat atomik.
    createAccount(username, passwordHash, tamerName) {
      db.exec('BEGIN IMMEDIATE');
      try {
        const acc = q.insertAccount.run(username, passwordHash).lastInsertRowid;
        const tamer = q.insertTamer.run(acc, tamerName).lastInsertRowid;
        q.insertDigimon.run(tamer);
        db.exec('COMMIT');
        return { accountId: Number(acc), tamerId: Number(tamer) };
      } catch (err) {
        try { db.exec('ROLLBACK'); } catch { /* sudah rollback */ }
        const f = dupField(err);
        if (f) throw new DuplicateError(f);
        throw err;
      }
    },

    touchLogin: (accountId) => q.touchLogin.run(accountId),

    loadProfile(accountId) {
      const tamer = q.tamerByAccount.get(accountId);
      if (!tamer) return null;
      return {
        tamer,
        digimon: q.digimon.get(tamer.TamerID) || null,
        worldPhasingFlags: q.flags.all(tamer.TamerID),
        bossClears: q.bossClears.all(tamer.TamerID),
        inventory: q.inventory.all(tamer.TamerID),
      };
    },

    savePosition: (tamerId, mapId, x, y, z) => q.savePos.run(mapId, x, y, z, tamerId),
  };
}

module.exports = { openDb, createStore, DuplicateError };
