-- Creates the logs database and enables TimescaleDB on first initialization.
-- Mounted as /docker-entrypoint-initdb.d/01-logs-db.sql in docker-compose.
CREATE DATABASE networker_logs OWNER networker;

-- Enable TimescaleDB on the logs database
\c networker_logs
CREATE EXTENSION IF NOT EXISTS timescaledb CASCADE;
