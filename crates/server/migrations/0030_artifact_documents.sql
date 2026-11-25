-- An artifact is now a document that owns files: a type, Markdown content, an
-- author and an updated time, plus any number of files. Existing rows become
-- type 'file' with their single file attached and no content. The legacy file
-- columns stay on the row so task files and old readers keep working.
alter table artifacts add column artifact_type text not null default 'file';
alter table artifacts add column content text not null default '';
alter table artifacts add column author text not null default '';
alter table artifacts add column author_is_agent integer not null default 0;
alter table artifacts add column updated_at text not null default '';
update artifacts set updated_at = created_at;

create table artifact_files (
    id blob primary key,
    artifact_id blob not null references artifacts(id) on delete cascade,
    filename text not null,
    mime text not null default 'application/octet-stream',
    size integer not null default 0,
    sha256 text not null default '',
    created_at text not null
);
create index artifact_files_by_artifact on artifact_files(artifact_id);

-- A migrated file keeps its artifact's id, so `/artifacts/<id>` still serves it.
insert into artifact_files (id, artifact_id, filename, mime, size, sha256, created_at)
select id, id, filename, mime, size, sha256, created_at from artifacts where filename <> '';
