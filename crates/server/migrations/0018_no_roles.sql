-- A turn is a turn. The server kept a role on every run and a binding per
-- project, which meant two places could disagree about who works a status. The
-- workflow document names the agents, and a finished run says what it did
-- through what it produced: a head that a review points at, or a verdict that
-- judged the head before it.
--
-- Nothing reads either of these any more, so they go rather than linger as a
-- column no query mentions.
drop table if exists project_agents;

alter table runs drop column role;
