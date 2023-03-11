use crate::model::{
    delegator::Delegator, keystore::Keystore, settings::Settings, signer::Signer,
    status_messages::StatusMessages,
};
use nostr::prelude::Keys;

#[derive(Clone, Debug)]
pub(crate) enum Action {
    DelegateDeeGenerate,
    DelegateSign,
    KeysClearNoConfirm,
    KeysClear,
    KeysGenerate,
    KeysImportPubkey,
    KeysImportSecretkey,
    KeysLoad,
    KeysSave,
    KeysUnlock,
    ConfirmationYes,
    ConfirmationNo,
    SignerConnect,
    SignerDisconnect,
    SignerPendingIgnoreFirst,
    SignerPendingProcessFirst,
}

/// Modal dialogs
#[derive(Clone)]
pub(crate) enum Modal {
    Confirmation(Confirmation),
}

#[derive(Clone)]
pub(crate) enum Confirmation {
    KeysClearBeforeAction(Option<Action>),
}

#[readonly::make]
pub(crate) struct KeystrModel {
    // app_id: Keys,
    pub own_keys: Keystore,
    pub delegator: Delegator,
    pub signer: Signer,
    pub status: StatusMessages,
    pub settings: Settings,
    #[readonly]
    modal: Option<Modal>,
}

impl KeystrModel {
    pub fn new() -> Self {
        let app_id = Keys::generate();
        Self {
            // app_id: app_id.clone(),
            own_keys: Keystore::new(),
            delegator: Delegator::new(),
            signer: Signer::new(&app_id),
            status: StatusMessages::new(),
            settings: Settings::default(),
            modal: None,
        }
    }

    // Create and init model
    pub fn init() -> Self {
        let mut model = Self::new();
        model.status.set("Keystr started");
        //. Try load settings
        if let Ok(sett) = Settings::load() {
            model.settings = sett;
        }
        //. Try load keys
        if model.settings.security.allows_persist() {
            model.action(Action::KeysLoad);
        }
        model
    }

    pub fn action(&mut self, action: Action) {
        match action {
            Action::DelegateDeeGenerate => self.delegator.generate_random_delegatee(),
            Action::DelegateSign => {
                match self.own_keys.get_keys() {
                    Err(e) => self.status.set_error(&e.to_string()),
                    Ok(keys) => match self.delegator.create_delegation(&keys) {
                        Err(e) => self.status.set_error(&e.to_string()),
                        Ok(_) => self.status.set("Delegation created"),
                    },
                };
            }
            Action::KeysClearNoConfirm => {
                self.own_keys.clear();
                self.status.set("Keys cleared");
            }
            Action::KeysClear => {
                if self.own_keys.keys_is_set() {
                    self.modal = Some(Modal::Confirmation(Confirmation::KeysClearBeforeAction(
                        None,
                    )));
                } else {
                    self.action(Action::KeysClearNoConfirm);
                }
            }
            Action::KeysGenerate => {
                if self.own_keys.keys_is_set() {
                    self.modal = Some(Modal::Confirmation(Confirmation::KeysClearBeforeAction(
                        Some(Action::KeysGenerate),
                    )));
                } else {
                    self.modal = None;
                    self.own_keys.generate();
                    self.status.set("New keypair generated");
                }
            }
            Action::KeysImportPubkey => {
                match self
                    .own_keys
                    .import_public_key(&self.own_keys.public_key_input.clone())
                {
                    Err(e) => self.status.set_error(&e.to_string()),
                    Ok(_) => self.status.set("Public key imported"),
                };
                // cleanup
                self.own_keys.public_key_input = String::new();
            }
            Action::KeysImportSecretkey => {
                self.own_keys.import_secret_key_action(&mut self.status);
            }
            Action::KeysLoad => {
                if self.own_keys.keys_is_set() {
                    self.modal = Some(Modal::Confirmation(Confirmation::KeysClearBeforeAction(
                        Some(Action::KeysLoad),
                    )));
                } else {
                    self.own_keys
                        .load_action(&self.settings.security, &mut self.status);
                }
            }
            Action::KeysSave => {
                self.own_keys
                    .save_action(&self.settings.security, &mut self.status);
            }
            Action::KeysUnlock => self
                .own_keys
                .unlock_secret_key_action(&self.settings.security, &mut self.status),
            Action::ConfirmationYes => {
                if let Some(Modal::Confirmation(conf)) = &self.modal {
                    match conf {
                        Confirmation::KeysClearBeforeAction(opt_next_action) => {
                            let prev_next_action = opt_next_action.clone();
                            self.modal = None;
                            self.action(Action::KeysClearNoConfirm);
                            if let Some(next_action) = prev_next_action {
                                self.action(next_action);
                            }
                        }
                    }
                }
            }
            Action::ConfirmationNo => {
                if let Some(Modal::Confirmation(_conf)) = &self.modal {
                    self.modal = None;
                }
            }
            Action::SignerConnect => match self.own_keys.get_signer() {
                Err(_) => self.status.set("Key pair is not loaded or unlocked!"),
                Ok(signer) => {
                    self.signer.connect_action(signer, &mut self.status);
                }
            },
            Action::SignerDisconnect => {
                self.signer.disconnect_action(&mut self.status);
            }
            Action::SignerPendingIgnoreFirst => {
                self.signer.pending_ignore_first_action(&mut self.status);
            }
            Action::SignerPendingProcessFirst => {
                self.signer.pending_process_first_action(&mut self.status);
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_clear_generate_confirmation() {
        let mut m = KeystrModel::new();
        assert_eq!(m.own_keys.keys_is_set(), false);
        assert!(m.modal.is_none());

        // generate
        m.action(Action::KeysGenerate);
        assert_eq!(m.own_keys.keys_is_set(), true);
        assert!(m.modal.is_none());

        // clear requires confirmation
        m.action(Action::KeysClear);
        assert_eq!(m.own_keys.keys_is_set(), true);
        assert!(m.modal.is_some());

        // confirmation No does not change it
        m.action(Action::ConfirmationNo);
        assert_eq!(m.own_keys.keys_is_set(), true);
        assert!(m.modal.is_none());

        // clear requires confirmation
        m.action(Action::KeysClear);
        assert_eq!(m.own_keys.keys_is_set(), true);
        assert!(m.modal.is_some());

        // confirmation Yes performs clean
        m.action(Action::ConfirmationYes);
        assert_eq!(m.own_keys.keys_is_set(), false);
        assert!(m.modal.is_none());

        // clear works now
        m.action(Action::KeysClear);
        assert_eq!(m.own_keys.keys_is_set(), false);
        assert!(m.modal.is_none());
    }
}
