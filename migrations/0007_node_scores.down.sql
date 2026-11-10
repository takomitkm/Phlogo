-- 0007_node_scores (down)
DELETE FROM configs WHERE key='score_min_samples';
DROP TABLE IF EXISTS node_scores;
