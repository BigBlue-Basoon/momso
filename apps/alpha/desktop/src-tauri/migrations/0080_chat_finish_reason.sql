ALTER TABLE chat_messages ADD COLUMN finish_reason TEXT CHECK (finish_reason IS NULL OR finish_reason IN ('completed','length_limit','turn_limit','cancelled','error','refusal'));
