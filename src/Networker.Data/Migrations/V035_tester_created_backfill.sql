INSERT INTO vm_lifecycle (
    project_id, resource_type, resource_id, resource_name,
    cloud, region, vm_size, vm_name, vm_resource_id,
    cloud_connection_id,
    event_type, event_time, triggered_by, metadata
)
SELECT
    t.project_id, 'tester', t.tester_id, t.name,
    t.cloud, t.region, t.vm_size, t.vm_name, t.vm_resource_id,
    t.cloud_connection_id,
    'created', t.created_at, t.created_by,
    jsonb_build_object('source', 'v035-backfill')
FROM project_tester t
WHERE NOT EXISTS (
    SELECT 1 FROM vm_lifecycle v
    WHERE v.resource_type = 'tester'
      AND v.resource_id   = t.tester_id
      AND v.event_type    = 'created'
);
