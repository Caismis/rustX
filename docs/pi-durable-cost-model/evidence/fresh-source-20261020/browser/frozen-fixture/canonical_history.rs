fn canonical_history(store: &SqliteConversationStore) -> Vec<MessageBlock> {
        let cut = store
            .observation_read_cut()
            .expect("native history cut");
        let mut offset = 0;
        let mut messages = Vec::new();
        loop {
            let page = store
                .message_header_page(&cut, Some(offset), 32)
                .expect("bounded canonical history page");
            assert_eq!(page.cut, cut);
            assert_eq!(page.offset, offset);
            assert_eq!(page.total, cut.ledger_through);
            assert!(page.headers.len() <= 32);
            let ids: Vec<_> = page
                .headers
                .iter()
                .enumerate()
                .map(|(index, header)| {
                    assert_eq!(
                        header.ledger_position,
                        Some(offset + u64::try_from(index).expect("bounded header ordinal") + 1)
                    );
                    header
                        .identity
                        .clone()
                        .expect("small original fixture identity")
                })
                .collect();
            let originals = store
                .load_messages(&ids)
                .expect("selected original bodies");
            assert_eq!(originals.len(), ids.len());
            for (original, identity) in originals.iter().zip(&ids) {
                assert_eq!(original.id(), identity);
            }
            messages.extend(originals);
            offset += u64::try_from(page.headers.len()).expect("bounded page count");
            assert_eq!(page.next_offset, (offset < page.total).then_some(offset));
            if page.next_offset.is_none() {
                assert_eq!(u64::try_from(messages.len()).unwrap(), cut.ledger_through);
                return messages;
            }
            assert!(!page.headers.is_empty(), "native continuation advances");
        }
    }

