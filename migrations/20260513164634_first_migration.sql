-- Add migration script here

CREATE TABLE IF NOT EXISTS `files` (
    `id` VARCHAR NOT NULL UNIQUE,
    `hash` VARCHAR NOT NULL UNIQUE,
    `size` INTEGER NOT NULL,
    `lastAccessedOn` INTEGER,

    PRIMARY KEY (`id`)
);