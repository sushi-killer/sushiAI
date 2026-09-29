## orchd: shutdown keeps tasks resumable

- Shutting orchd down (an upgrade or SIGTERM) leaves running, drafting and queued tasks resumable instead of stopping them, and the app replaces a rebuilt orchd even while tasks are running.
