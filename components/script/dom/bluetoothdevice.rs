/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use bluetooth_traits::{
    BluetoothCharacteristicMsg, BluetoothDescriptorMsg, BluetoothRequest, BluetoothResponse,
    BluetoothServiceMsg,
};
use dom_struct::dom_struct;
use ipc_channel::ipc::IpcSender;
use profile_traits::ipc;

use crate::dom::bindings::cell::DomRefCell;
use crate::dom::bindings::codegen::Bindings::BluetoothDeviceBinding::BluetoothDeviceMethods;
use crate::dom::bindings::codegen::Bindings::BluetoothRemoteGATTServerBinding::BluetoothRemoteGATTServerMethods;
use crate::dom::bindings::error::{Error, ErrorResult};
use crate::dom::bindings::inheritance::Castable;
use crate::dom::bindings::reflector::{reflect_dom_object, DomObject};
use crate::dom::bindings::root::{Dom, DomRoot, MutNullableDom};
use crate::dom::bindings::str::DOMString;
use crate::dom::bluetooth::{response_async, AsyncBluetoothListener, Bluetooth};
use crate::dom::bluetoothcharacteristicproperties::BluetoothCharacteristicProperties;
use crate::dom::bluetoothremotegattcharacteristic::BluetoothRemoteGATTCharacteristic;
use crate::dom::bluetoothremotegattdescriptor::BluetoothRemoteGATTDescriptor;
use crate::dom::bluetoothremotegattserver::BluetoothRemoteGATTServer;
use crate::dom::bluetoothremotegattservice::BluetoothRemoteGATTService;
use crate::dom::eventtarget::EventTarget;
use crate::dom::globalscope::GlobalScope;
use crate::dom::promise::Promise;
use crate::realms::InRealm;

// https://webbluetoothcg.github.io/web-bluetooth/#bluetoothdevice
#[dom_struct]
pub struct BluetoothDevice {
    eventtarget: EventTarget,
    id: DOMString,
    name: Option<DOMString>,
    gatt: MutNullableDom<BluetoothRemoteGATTServer>,
    context: Dom<Bluetooth>,
    attribute_instance_map: (
        DomRefCell<HashMap<String, Dom<BluetoothRemoteGATTService>>>,
        DomRefCell<HashMap<String, Dom<BluetoothRemoteGATTCharacteristic>>>,
        DomRefCell<HashMap<String, Dom<BluetoothRemoteGATTDescriptor>>>,
    ),
    watching_advertisements: Cell<bool>,
}

impl BluetoothDevice {
    pub fn new_inherited(
        id: DOMString,
        name: Option<DOMString>,
        context: &Bluetooth,
    ) -> BluetoothDevice {
        BluetoothDevice {
            eventtarget: EventTarget::new_inherited(),
            id: id,
            name: name,
            gatt: Default::default(),
            context: Dom::from_ref(context),
            attribute_instance_map: (
                DomRefCell::new(HashMap::new()),
                DomRefCell::new(HashMap::new()),
                DomRefCell::new(HashMap::new()),
            ),
            watching_advertisements: Cell::new(false),
        }
    }

    pub fn new(
        global: &GlobalScope,
        id: DOMString,
        name: Option<DOMString>,
        context: &Bluetooth,
    ) -> DomRoot<BluetoothDevice> {
        reflect_dom_object(
            Box::new(BluetoothDevice::new_inherited(id, name, context)),
            global,
        )
    }

    pub fn get_gatt(&self) -> DomRoot<BluetoothRemoteGATTServer> {
        self.gatt
            .or_init(|| BluetoothRemoteGATTServer::new(&self.global(), self))
    }

    fn get_context(&self) -> DomRoot<Bluetooth> {
        DomRoot::from_ref(&self.context)
    }

    pub fn get_or_create_service(
        &self,
        service: &BluetoothServiceMsg,
        server: &BluetoothRemoteGATTServer,
    ) -> DomRoot<BluetoothRemoteGATTService> {
        let (ref service_map_ref, _, _) = self.attribute_instance_map;
        let mut service_map = service_map_ref.borrow_mut();
        if let Some(existing_service) = service_map.get(&service.instance_id) {
            return DomRoot::from_ref(&existing_service);
        }
        let bt_service = BluetoothRemoteGATTService::new(
            &server.global(),
            &server.Device(),
            DOMString::from(service.uuid.clone()),
            service.is_primary,
            service.instance_id.clone(),
        );
        service_map.insert(service.instance_id.clone(), Dom::from_ref(&bt_service));
        return bt_service;
    }

    pub fn get_or_create_characteristic(
        &self,
        characteristic: &BluetoothCharacteristicMsg,
        service: &BluetoothRemoteGATTService,
    ) -> DomRoot<BluetoothRemoteGATTCharacteristic> {
        let (_, ref characteristic_map_ref, _) = self.attribute_instance_map;
        let mut characteristic_map = characteristic_map_ref.borrow_mut();
        if let Some(existing_characteristic) = characteristic_map.get(&characteristic.instance_id) {
            return DomRoot::from_ref(&existing_characteristic);
        }
        let properties = BluetoothCharacteristicProperties::new(
            &service.global(),
            characteristic.broadcast,
            characteristic.read,
            characteristic.write_without_response,
            characteristic.write,
            characteristic.notify,
            characteristic.indicate,
            characteristic.authenticated_signed_writes,
            characteristic.reliable_write,
            characteristic.writable_auxiliaries,
        );
        let bt_characteristic = BluetoothRemoteGATTCharacteristic::new(
            &service.global(),
            service,
            DOMString::from(characteristic.uuid.clone()),
            &properties,
            characteristic.instance_id.clone(),
        );
        characteristic_map.insert(
            characteristic.instance_id.clone(),
            Dom::from_ref(&bt_characteristic),
        );
        return bt_characteristic;
    }

    pub fn is_represented_device_null(&self) -> bool {
        let (sender, receiver) = ipc::channel(self.global().time_profiler_chan().clone()).unwrap();
        self.get_bluetooth_thread()
            .send(BluetoothRequest::IsRepresentedDeviceNull(
                self.Id().to_string(),
                sender,
            ))
            .unwrap();
        receiver.recv().unwrap()
    }

    pub fn get_or_create_descriptor(
        &self,
        descriptor: &BluetoothDescriptorMsg,
        characteristic: &BluetoothRemoteGATTCharacteristic,
    ) -> DomRoot<BluetoothRemoteGATTDescriptor> {
        let (_, _, ref descriptor_map_ref) = self.attribute_instance_map;
        let mut descriptor_map = descriptor_map_ref.borrow_mut();
        if let Some(existing_descriptor) = descriptor_map.get(&descriptor.instance_id) {
            return DomRoot::from_ref(&existing_descriptor);
        }
        let bt_descriptor = BluetoothRemoteGATTDescriptor::new(
            &characteristic.global(),
            characteristic,
            DOMString::from(descriptor.uuid.clone()),
            descriptor.instance_id.clone(),
        );
        descriptor_map.insert(
            descriptor.instance_id.clone(),
            Dom::from_ref(&bt_descriptor),
        );
        return bt_descriptor;
    }

    fn get_bluetooth_thread(&self) -> IpcSender<BluetoothRequest> {
        self.global().as_window().bluetooth_thread()
    }

    // https://webbluetoothcg.github.io/web-bluetooth/#clean-up-the-disconnected-device
    #[allow(crown::unrooted_must_root)]
    pub fn clean_up_disconnected_device(&self) {
        // Step 1.
        self.get_gatt().set_connected(false);

        // TODO: Step 2: Implement activeAlgorithms internal slot for BluetoothRemoteGATTServer.

        // Step 3: We don't need `context`, we get the attributeInstanceMap from the device.
        // https://github.com/WebBluetoothCG/web-bluetooth/issues/330

        // Step 4.
        let mut service_map = self.attribute_instance_map.0.borrow_mut();
        let service_ids = service_map.drain().map(|(id, _)| id).collect();

        let mut characteristic_map = self.attribute_instance_map.1.borrow_mut();
        let characteristic_ids = characteristic_map.drain().map(|(id, _)| id).collect();

        let mut descriptor_map = self.attribute_instance_map.2.borrow_mut();
        let descriptor_ids = descriptor_map.drain().map(|(id, _)| id).collect();

        // Step 5, 6.4, 7.
        // TODO: Step 6: Implement `active notification context set` for BluetoothRemoteGATTCharacteristic.
        let _ = self
            .get_bluetooth_thread()
            .send(BluetoothRequest::SetRepresentedToNull(
                service_ids,
                characteristic_ids,
                descriptor_ids,
            ));

        // Step 8.
        self.upcast::<EventTarget>()
            .fire_bubbling_event(atom!("gattserverdisconnected"));
    }

    // https://webbluetoothcg.github.io/web-bluetooth/#garbage-collect-the-connection
    #[allow(crown::unrooted_must_root)]
    pub fn garbage_collect_the_connection(&self) -> ErrorResult {
        // Step 1: TODO: Check if other systems using this device.

        // Step 2.
        let context = self.get_context();
        for (id, device) in context.get_device_map().borrow().iter() {
            // Step 2.1 - 2.2.
            if id == &self.Id().to_string() {
                if device.get_gatt().Connected() {
                    return Ok(());
                }
                // TODO: Step 2.3: Implement activeAlgorithms internal slot for BluetoothRemoteGATTServer.
            }
        }

        // Step 3.
        let (sender, receiver) = ipc::channel(self.global().time_profiler_chan().clone()).unwrap();
        self.get_bluetooth_thread()
            .send(BluetoothRequest::GATTServerDisconnect(
                String::from(self.Id()),
                sender,
            ))
            .unwrap();
        receiver.recv().unwrap().map_err(Error::from)
    }
}

impl BluetoothDeviceMethods for BluetoothDevice {
    // https://webbluetoothcg.github.io/web-bluetooth/#dom-bluetoothdevice-id
    fn Id(&self) -> DOMString {
        self.id.clone()
    }

    // https://webbluetoothcg.github.io/web-bluetooth/#dom-bluetoothdevice-name
    fn GetName(&self) -> Option<DOMString> {
        self.name.clone()
    }

    // https://webbluetoothcg.github.io/web-bluetooth/#dom-bluetoothdevice-gatt
    fn GetGatt(&self) -> Option<DomRoot<BluetoothRemoteGATTServer>> {
        // Step 1.
        if self
            .global()
            .as_window()
            .bluetooth_extra_permission_data()
            .allowed_devices_contains_id(self.id.clone()) &&
            !self.is_represented_device_null()
        {
            return Some(self.get_gatt());
        }
        // Step 2.
        None
    }

    // https://webbluetoothcg.github.io/web-bluetooth/#dom-bluetoothdevice-watchadvertisements
    fn WatchAdvertisements(&self, comp: InRealm) -> Rc<Promise> {
        let p = Promise::new_in_current_realm(comp);
        let sender = response_async(&p, self);
        // TODO: Step 1.
        // Note: Steps 2 - 3 are implemented in components/bluetooth/lib.rs in watch_advertisements function
        // and in handle_response function.
        self.get_bluetooth_thread()
            .send(BluetoothRequest::WatchAdvertisements(
                String::from(self.Id()),
                sender,
            ))
            .unwrap();
        return p;
    }

    // https://webbluetoothcg.github.io/web-bluetooth/#dom-bluetoothdevice-unwatchadvertisements
    fn UnwatchAdvertisements(&self) {
        // Step 1.
        self.watching_advertisements.set(false)
        // TODO: Step 2.
    }

    // https://webbluetoothcg.github.io/web-bluetooth/#dom-bluetoothdevice-watchingadvertisements
    fn WatchingAdvertisements(&self) -> bool {
        self.watching_advertisements.get()
    }

    // https://webbluetoothcg.github.io/web-bluetooth/#dom-bluetoothdeviceeventhandlers-ongattserverdisconnected
    event_handler!(
        gattserverdisconnected,
        GetOngattserverdisconnected,
        SetOngattserverdisconnected
    );
}

impl AsyncBluetoothListener for BluetoothDevice {
    fn handle_response(&self, response: BluetoothResponse, promise: &Rc<Promise>) {
        match response {
            // https://webbluetoothcg.github.io/web-bluetooth/#dom-bluetoothdevice-unwatchadvertisements
            BluetoothResponse::WatchAdvertisements(_result) => {
                // Step 3.1.
                self.watching_advertisements.set(true);
                // Step 3.2.
                promise.resolve_native(&());
            },
            _ => promise.reject_error(Error::Type("Something went wrong...".to_owned())),
        }
    }
}
