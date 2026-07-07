# Dirty Source Trees block updates

When a Source Tree has local uncommitted changes, update operations will refuse to proceed by default. The tool will show Simplified Chinese choices such as viewing the changes, creating a local backup before continuing, or skipping that Source Tree. We will not automatically stash changes, because hidden stash state is hard for ordinary users to understand and recover from.
