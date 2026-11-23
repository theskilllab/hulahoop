-- What an agent said when it finished, kept on the run.
--
-- A reviewer's message became a review row. Every other role's closing message
-- went nowhere, so the next turn of a status ran without knowing what the turn
-- before it decided. The summary is that message: shown as a comment, and handed
-- to the next run as its `parent`.

alter table runs add column summary text;
