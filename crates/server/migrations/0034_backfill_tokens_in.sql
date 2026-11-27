-- `runs.tokens_in` now counts cached input too (fresh + cache read + cache write).
-- Every row that exists when this runs was written with fresh input only, so add
-- its cache columns once; rows written afterwards already carry the total.
update runs
set tokens_in = coalesce(tokens_in, 0) + coalesce(cache_read_tokens, 0) + coalesce(cache_write_tokens, 0)
where cache_read_tokens is not null or cache_write_tokens is not null;
