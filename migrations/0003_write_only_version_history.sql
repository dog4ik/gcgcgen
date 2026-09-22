-- Version rows are written and never read back, so there is no lookup index
-- and no note for a caller to supply.
create table if not exists integration_versions (
    integration_id integer not null references integrations (id) on delete cascade,
    version        integer not null,
    spec           text    not null,
    created_at     text    not null,
    primary key (integration_id, version)
) strict;
