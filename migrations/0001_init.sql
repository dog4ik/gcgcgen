-- Integration documents.
--
-- One row per integration, holding the whole spec as a JSON document. It is
-- authored and edited as a unit, so normalising it into methods/requests/
-- mappings tables would buy queryability nobody needs at the cost of a
-- six-join load and a painful editor round-trip.
--
-- Note what is *not* here: credentials. They arrive in the `settings` bucket
-- of every inbound request and are never persisted.
create table integrations (
    id         integer primary key autoincrement,
    key        text    not null unique,
    name       text    not null,
    spec       text    not null,
    version    integer not null default 1,
    created_at text    not null,
    updated_at text    not null
) strict;

-- Every save appends here, so rollback is a copy rather than a migration.
create table integration_versions (
    integration_id integer not null references integrations (id) on delete cascade,
    version        integer not null,
    spec           text    not null,
    created_at     text    not null,
    note           text,
    primary key (integration_id, version)
) strict;

create index integration_versions_by_time on integration_versions (integration_id, created_at desc);
