-- Creates the logs database on first PostgreSQL initialization.
-- Mounted as /docker-entrypoint-initdb.d/01-logs-db.sql in docker-compose.
CREATE DATABASE networker_logs OWNER networker;
