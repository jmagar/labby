# Stash: Fork Artifact + Patch Artifact
# Project: lab
# Date: 04/24/2026 - 11:30 EST
# Last Updated: 04/24/2026 - 11:36 EST


## Problem

Plugins ship with many different Artifacts. And theyre installed within their own subdirectory seperate from our regular Claude Artifacts in ~/.claude/

Often times you'll need to slightly tweak how the Agent, Skill, etc works.

Now you're stuck in this awkward position:
  - Do you copy the skill into ~/.claude/skills?
    - Then you're duplicating, unless you delete it - then you're going to have an error lingering in Claude Code complaining about the skill not being found for the plugn
    - It will also be recreated on plugin update.
  - Do you edit it in place? 
    - You can't - well, you can - but when the plugin updates its overriden
  - Do you fork the whole plugin repo - point Claude Code at your local repo for that plugin, and then pull new upstream updates?
    - Sure that'll work - but thats quite a bit of rigamorale just to change one line in a markdown file...


## Feature

I want to create a feature for the Marketplace that tracks changes to any of the files distributed with the plugin and output a diff of the changes.

We will then track diffs of all installed plugins, and when a plugin update is released, we can show the user the diff of their changes vs the new version of the plugin. The user can then choose to pull in the new changes from the plugin author, or keep their changes. We can allow the user to easily apply their changes on top of the new plugin version, and then save that as a new version of the plugin in their Stash. 

This way they can easily keep their changes while also pulling in updates from the plugin author. We should allow the user to:
    - View the diff of their changes vs the new plugin version
    - Choose to pull in the new changes from the plugin author, or keep their changes
    - Easily apply their changes on top of the new plugin version, and then save that as a new version of the artifact/plugin in their Stash
    - When they save a new version of the plugin in their Stash, we should also allow them to easily deploy that new version to any / all of their devices.
    - User should be able to configure whether they want to be notified of plugin updates, and whether they want to automatically pull in updates from the plugin author, or keep their changes. 
    - If the user wants to keep their changes on plugin update - we should still pull the newest copy to have on hand.
    - If the user wants to pull in updates from the plugin author - we should still keep a copy of their changes on hand, and allow them to easily apply those changes on top of the new plugin version if they want to.

Now we can already quickly edit a file from plugins, save it to our Stash, and then deploy it to any / all of our devices. But this still falls apart a bit if the plugin is updated and the author of the Plugin has made changes that you'd like to keep.

We want to reduce the friction of making changes to plugin files, and then being able to easily pull in updates from the plugin author without losing your changes.


## Proposal
    New Features:
        1. Artifact Diffs + Merge
            - Track changes to all files from all plugins from all marketplaces
            - When a plugin update is released, show the user a diff of their changes vs the new version of the plugin. The user can then choose to pull in the new changes from the plugin author, or keep their changes. We can allow the user to easily apply their changes on top of the new plugin version, and then save that as a new version of the artifact in their Stash.
            - When they save a new version of the plugin in their Stash, we should also allow them to easily deploy that new version to any / all of their devices.
            - User should be able to configure whether they want to be notified of plugin updates, and whether they want to automatically pull in updates from the plugin author, or keep their changes. 
            - If the user wants to keep their changes on plugin update - we should still pull the newest copy to have on hand.
            - If the user wants to pull in updates from the plugin author - we should still keep a copy of their changes on hand, and allow them to easily apply those changes on top of the new plugin version if they want to.
            
        3. Fork Artifact

        Forking (The Artifact) - The act of copying an artifact from a plugin in the Marketplace, and then saving that copied version to your Stash whilst still keeping a link to the original artifact in the plugin. This way you can easily pull in updates from the plugin author, and also easily apply your changes on top of new versions of the plugin.
        
            - Allow the user to fork any artifact from any plugin in the Marketplace, and then save that forked version to their Stash. 
            - Should work just like forking a repo on GitHub - the user can make changes to the forked artifact, and then save it as a new version in their Stash.
            - When a User makes changes to a forked artifact - it's called patching the artifact - we should track those changes as well, and allow the user to easily apply those changes on top of new versions of the plugin that they forked from.
 
        4. Patch Artifact

        Patching (The Artifact) - The act of making changes to a forked artifact, and then saving those changes as a new version in your Stash. This way you can easily keep track of the changes you've made to the artifact, and also easily apply those changes on top of new versions of the plugin that you forked from.

            - Allow the user to patch any forked artifact in their Stash, and then save those changes as a new version in their Stash.
            - When a User makes changes to a patched artifact - we should track those changes as well, and allow the user to easily apply those changes on top of new versions of the plugin that they forked from.
            - We should think of our forked artifacts as being on a separate branch from the original artifact in the plugin - we can call it the "Stash Branch" - and then when a new version of the plugin is released, we can show the user a diff of the changes between the "Stash Branch" and the "Plugin Branch" - and then allow them to easily merge those changes together, or keep them separate.
            - This system we're creating is essentially drop-in configs for plugins - you can fork an artifact from a plugin, make changes to it, and then easily pull in updates from the plugin author without losing your changes. It's a way to have your cake and eat it too when it comes to customizing plugins.

## Required Capabilities

1. Easily change plugin files without worrying about losing your changes on plugin update
2. Easily pull in updates from the plugin author without losing your changes
3. Easily keep track of the changes you've made to plugin files, and also easily apply those
changes on top of new versions of the plugin that you forked from.

## Implementation Boundary

Forked marketplace artifacts are durable stash components. The Marketplace
surface is the user-facing upstream workflow: fork, list, update preview, update
apply, reset, and unfork. The Stash surface is the durable artifact library:
workspace, immutable revisions, provider sync, export, and deploy handoff.


## Bonus Points
1. Push notifications


## NOT IN SCOPE
1. Version control system for plugins - we can use git for this, but we don't want to require users to have git installed or know how to use it. We want to abstract away the complexity of version control and just provide a simple interface for tracking changes and pulling in updates from the plugin author.
