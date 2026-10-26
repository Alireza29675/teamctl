import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import react from '@astrojs/react';
import releaseSignupDevPlugin from './tools/release-signup-dev.mjs';

export default defineConfig({
  site: 'https://teamctl.run',
  vite: { plugins: [releaseSignupDevPlugin()] },
  integrations: [
    react(),
    starlight({
      title: 'TeamCTL',
      description: 'An open-source platform for autonomous teams of agents. Define their roles, structure and conversations. Run your team in the background and share its setup.',
      logo: {
        src: './public/brand/teamctl-head.svg',
        alt: '',
      },
      favicon: '/favicon.svg',
      head: [
        { tag: 'link', attrs: { rel: 'icon', href: '/favicon.ico', sizes: '16x16 32x32 48x48' } },
        { tag: 'meta', attrs: { property: 'og:image', content: 'https://teamctl.run/brand/social-cover.png' } },
        { tag: 'meta', attrs: { property: 'og:image:alt', content: 'TeamCTL — Create autonomous teams of agents.' } },
        { tag: 'meta', attrs: { name: 'twitter:card', content: 'summary_large_image' } },
        { tag: 'meta', attrs: { name: 'twitter:image', content: 'https://teamctl.run/brand/social-cover.png' } },
      ],
      social: {
        github: 'https://github.com/Alireza29675/teamctl',
      },
      editLink: {
        baseUrl: 'https://github.com/Alireza29675/teamctl/edit/main/docs/',
      },
      sidebar: [
        { label: 'Getting started', link: '/getting-started/' },
        {
          label: 'Operating',
          items: [
            { label: 'Cost & rate limits', link: '/cost/' },
            { label: 'Coordination policy', link: '/coordination-policy/' },
          ],
        },
        {
          label: 'Concepts',
          items: [
            { label: 'Designing an agent team', link: '/concepts/teams/' },
            { label: 'Projects', link: '/concepts/projects/' },
            { label: 'Channels', link: '/concepts/channels/' },
            { label: 'Runtimes', link: '/concepts/runtimes/' },
            { label: 'Bridges', link: '/concepts/bridges/' },
            { label: 'The .team/ folder', link: '/concepts/team-folder/' },
            { label: 'Interfaces', link: '/concepts/interfaces/' },
            { label: 'HITL', link: '/concepts/hitl/' },
            { label: 'Rate limits', link: '/concepts/rate-limits/' },
          ],
        },
        {
          label: 'Best practices',
          items: [
            { label: 'Ways of working', link: '/best-practices/ways-of-working/' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'team-compose.yaml', link: '/reference/team-compose-yaml/' },
            { label: 'teamctl CLI', link: '/reference/teamctl/' },
            { label: 'runtimes/*.yaml', link: '/reference/runtimes-yaml/' },
          ],
        },
        { label: 'Changelog', link: '/changelog/' },
        {
          label: 'Guides',
          items: [
            { label: 'Your first team', link: '/guides/first-team/' },
            { label: 'Multi-runtime teams', link: '/guides/multi-runtime/' },
            { label: 'Existing MCP servers', link: '/guides/existing-mcps/' },
            { label: 'Telegram bot setup', link: '/guides/telegram-bot/' },
            { label: 'Updating teamctl', link: '/guides/updating/' },
            { label: 'Bridges and HITL', link: '/guides/bridges-and-hitl/' },
            { label: 'Operating in production', link: '/guides/operating-in-production/' },
          ],
        },
        {
          label: 'Cookbook',
          items: [
            { label: 'Essentials walkthrough', link: '/cookbook/essentials-walkthrough/' },
            { label: 'Multi-agent ACLs in one project', link: '/cookbook/multi-agent/' },
            { label: 'Mixing runtimes in one team', link: '/cookbook/multi-runtime/' },
            { label: 'Two projects, one teamctl, with bridges', link: '/cookbook/two-projects/' },
            { label: 'Per-agent reasoning effort', link: '/cookbook/effort/' },
          ],
        },
      ],
      customCss: ['./src/styles/brand.css'],
    }),
  ],
});
