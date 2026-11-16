CREATE INDEX chat_search_page_order ON chats(archived_at, (pinned_order IS NULL), pinned_order, updated_at DESC, id DESC);
