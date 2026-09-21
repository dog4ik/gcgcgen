-- Integration documents.
create table if not exists integrations (
    id         integer primary key autoincrement,
    key        text    not null unique,
    name       text    not null,
    spec       text    not null,
    version    integer not null default 1,
    created_at text    not null,
    updated_at text    not null
) strict;

create table if not exists integration_versions (
    integration_id integer not null references integrations (id) on delete cascade,
    version        integer not null,
    spec           text    not null,
    created_at     text    not null,
    note           text,
    primary key (integration_id, version)
) strict;

create index if not exists integration_versions_by_time on integration_versions (integration_id, created_at desc);
