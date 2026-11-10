import type { RouteRecordRaw } from 'vue-router';

/**
 * paily admin routes (p6-02): Sources / Nodes / Scoring.
 * Endpoint mappings per docs/rebuild/07-p6-admin-ui/p6-02-pages.md.
 */
const routes: RouteRecordRaw[] = [
  {
    meta: {
      icon: 'lucide:list',
      order: 900,
      title: 'Paily',
    },
    name: 'PailyRoot',
    path: '/paily',
    children: [
      {
        meta: { title: 'Sources' },
        name: 'PailySources',
        path: 'sources',
        component: () => import('#/views/paily/sources.vue'),
      },
      {
        meta: { title: 'Nodes' },
        name: 'PailyNodes',
        path: 'nodes',
        component: () => import('#/views/paily/nodes.vue'),
      },
      {
        meta: { title: 'Scoring' },
        name: 'PailyScoring',
        path: 'scoring',
        component: () => import('#/views/paily/scoring.vue'),
      },
    ],
  },
];

export default routes;
