import type { RouteRecordRaw } from 'vue-router';

/**
 * paily admin routes (p6-02): Dashboard / Sources / Nodes / NodeDetail / Checks / Scoring / Subscriptions.
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
        meta: { title: 'Dashboard' },
        name: 'PailyDashboard',
        path: 'dashboard',
        component: () => import('#/views/paily/dashboard.vue'),
      },
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
        meta: { title: 'Node Detail', hideInMenu: true },
        name: 'PailyNodeDetail',
        path: 'nodes/:id',
        component: () => import('#/views/paily/node-detail.vue'),
      },
      {
        meta: { title: 'Checks' },
        name: 'PailyChecks',
        path: 'checks',
        component: () => import('#/views/paily/checks.vue'),
      },
      {
        meta: { title: 'Scoring' },
        name: 'PailyScoring',
        path: 'scoring',
        component: () => import('#/views/paily/scoring.vue'),
      },
      {
        meta: { title: 'Subscriptions' },
        name: 'PailySubscriptions',
        path: 'subscriptions',
        component: () => import('#/views/paily/subscriptions.vue'),
      },
      {
        meta: { title: 'Sponsors' },
        name: 'PailySponsors',
        path: 'sponsors',
        component: () => import('#/views/paily/sponsors.vue'),
      },
    ],
  },
];

export default routes;
