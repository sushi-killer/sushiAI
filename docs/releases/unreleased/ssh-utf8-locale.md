## Terminal

- Terminal sessions get a UTF-8 locale when the daemon starts without one (for example over ssh), so Cyrillic and other non-ASCII text copied from an agent CLI stays intact.
