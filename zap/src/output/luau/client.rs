use std::{cmp::max, collections::HashMap};

use crate::{
	config::{
		Config, EvCall, EvDecl, EvSource, EvType, FnDecl, NamespaceEntry, Parameter, TyDecl, UNRELIABLE_ORDER_NUMTY,
		YieldType,
	},
	irgen::{des, ser},
	output::{
		get_named_values, get_unnamed_values,
		luau::{event_queue_table_name, events_table_name, polling_queues_name},
	},
};

use super::Output;

struct ClientOutput<'src> {
	config: &'src Config<'src>,
	tabs: u32,
	buf: String,
	var_occurrences: HashMap<String, usize>,
}

impl Output for ClientOutput<'_> {
	fn push(&mut self, s: &str) {
		self.buf.push_str(s);
	}

	fn indent(&mut self) {
		self.tabs += 1;
	}

	fn dedent(&mut self) {
		self.tabs -= 1;
	}

	fn push_indent(&mut self) {
		for _ in 0..self.tabs {
			self.push("\t");
		}
	}
}

impl<'src> ClientOutput<'src> {
	pub fn new(config: &'src Config<'src>) -> Self {
		Self {
			config,
			tabs: 0,
			buf: String::new(),
			var_occurrences: HashMap::new(),
		}
	}

	fn push_studio(&mut self) {
		self.push_line("if not RunService:IsRunning() then");
		self.indent();

		self.push_line("local noop = function() end");

		self.push_line("return table.freeze({");
		self.indent();

		let fire = self.config.casing.with("Fire", "fire", "fire");
		let set_callback = self.config.casing.with("SetCallback", "setCallback", "set_callback");
		let iter = self.config.casing.with("Iter", "iter", "iter");
		let on = self.config.casing.with("On", "on", "on");
		let call = self.config.casing.with("Call", "call", "call");

		let send_events = self.config.casing.with("SendEvents", "sendEvents", "send_events");

		self.push_line(&format!("{send_events} = noop,"));

		self.config.traverse_namespaces(
			self,
			|this, diff| {
				for _ in 0..diff {
					this.dedent();
					this.push_line("}),");
				}
			},
			|this, path, entry| {
				let name = path.last().unwrap();
				this.push_line(&format!("{name} = table.freeze({{"));
				this.indent();

				match entry {
					NamespaceEntry::EvDecl(ev) => {
						if ev.from == EvSource::Client {
							this.push_line(&format!("{fire} = noop"));
						} else {
							match ev.call {
								EvCall::SingleSync | EvCall::SingleAsync => {
									this.push_line(&format!("{set_callback} = noop"))
								}
								EvCall::ManySync | EvCall::ManyAsync => this.push_line(&format!("{on} = noop")),
								EvCall::Polling => {
									this.push_line(&format!("{iter} = function()"));
									this.indent();
									this.push_line("return noop");
									this.dedent();
									this.push_line("end");
								}
							}
						}

						this.dedent();
						this.push_line("}),");
					}
					NamespaceEntry::FnDecl(fndecl) => {
						this.push_indent();
						this.push(&format!("{call} = "));

						match self.config.yield_type {
							YieldType::Yield => this.push("noop\n"),
							YieldType::Future => {
								this.push("function()\n");
								this.indent();
								this.push_line("return Future.new(function()");
								this.indent();
								this.push_line(&format!("error(\"{} called when game is not running\")", fndecl.name));
								this.dedent();
								this.push_line("end)");
								this.dedent();
								this.push_line("end");
							}
							YieldType::Promise => {
								this.push("function()\n");
								this.indent();
								this.push_line(&format!(
									"return Promise.reject(\"{} called when game is not running\")",
									fndecl.name
								));
								this.dedent();
								this.push_line("end");
							}
						}

						this.dedent();
						this.push_line("}),");
					}
					NamespaceEntry::Ns(..) => {}
				}
			},
		);

		self.dedent();
		self.push_line("}) :: Events");

		self.dedent();
		self.push_line("end");
	}

	fn push_tydecl(&mut self, tydecl: &TyDecl) {
		let ty = &*tydecl.ty.borrow();

		self.push_indent();
		if tydecl.path.is_empty() {
			self.push("export ");
		}
		self.push(&format!("type {tydecl} = "));
		self.push_ty(ty);
		self.push("\n");

		if tydecl.inline {
			return;
		}

		self.push_line(&format!("function types.write_{tydecl}(value: {tydecl})"));
		self.indent();
		let statements = &ser::generate(
			&[ty.clone()],
			&["value".to_string()],
			self.config.write_checks,
			&mut HashMap::new(),
		);
		self.push_stmts(statements);
		self.dedent();
		self.push_line("end");

		self.push_line(&format!("function types.read_{tydecl}()"));
		self.indent();
		self.push_line("local value;");
		let statements = &des::generate(&[ty.clone()], &["value".to_string()], false, &mut HashMap::new());
		self.push_stmts(statements);
		self.push_line("return value");
		self.dedent();
		self.push_line("end");
	}

	fn push_tydecls(&mut self) {
		for tydecl in &self.config.tydecls {
			self.push_tydecl(tydecl);
		}
	}

	fn push_event_loop_body(&mut self) {
		self.push_line("if outgoing_used ~= 0 then");
		self.indent();
		self.push_line("local buff = buffer.create(outgoing_used)");
		self.push_line("buffer.copy(buff, 0, outgoing_buff, 0, outgoing_used)");
		self.push("\n");
		self.push_line("reliable:FireServer(buff, outgoing_inst)");
		self.push("\n");
		self.push_line("outgoing_buff = buffer.create(64)");
		self.push_line("outgoing_used = 0");
		self.push_line("outgoing_size = 64");
		self.push_line("table.clear(outgoing_inst)");
		self.dedent();
		self.push_line("end");
		self.dedent();
	}

	fn push_event_loop(&mut self) {
		self.push("\n");

		let send_events = self.config.casing.with("SendEvents", "sendEvents", "send_events");

		self.push_line(&format!("local function {send_events}()"));
		self.indent();
		self.push_event_loop_body();
		self.push_line("end\n");

		if !self.config.manual_event_loop {
			self.push_line(&format!("RunService.Heartbeat:Connect({send_events})\n"));
		}
	}

	fn push_reliable_header(&mut self) {
		self.push_line("reliable.OnClientEvent:Connect(function(buff, inst)");
		self.indent();
		self.push_line("incoming_buff = buff");
		self.push_line("incoming_inst = inst");
		self.push_line("incoming_read = 0");
		self.push_line("incoming_ipos = 0");

		self.push_line("local len = buffer.len(buff)");
		self.push_line("while incoming_read < len do");

		self.indent();

		let client_reliable_ty = self.config.client_reliable_ty();

		self.push_line(&format!(
			"local id = buffer.read{}(buff, read({}))",
			client_reliable_ty,
			client_reliable_ty.size()
		));
	}

	fn get_values(&self, count: usize) -> String {
		if count > 0 {
			(1..=count)
				.map(|i| {
					if i == 1 {
						"value".to_string()
					} else {
						format!("value{i}")
					}
				})
				.collect::<Vec<_>>()
				.join(", ")
		} else {
			"value".to_string()
		}
	}

	fn push_polling_event(&mut self, ev: &EvDecl) {
		let id = ev.id;
		let arguments = get_unnamed_values("value", ev.data.len());
		let returns_length = arguments.len().max(1);

		self.push_line(&format!("local queue = {}[{id}]", polling_queues_name(ev)));
		self.push_line("-- `arguments` is a circular buffer.");
		self.push_line("-- `queue.arguments` can be replaced when it needs to grow.");
		self.push_line(
			"-- It's indexed like `arguments[((index - 1) % queue_size) + 1] because Luau has 1-based indexing.",
		);
		self.push_line("local arguments = queue.arguments");
		self.push_line("local queue_size = queue.queue_size");
		self.push_line("local read_cursor = queue.read_cursor");
		self.push_line("local write_cursor = queue.write_cursor");
		self.push_line(&format!(
			"local unwrapped_write_end_cursor = write_cursor + {returns_length}"
		));
		self.push_line("local write_end_cursor = ((unwrapped_write_end_cursor - 1) % queue_size) + 1");

		self.push_line("if (write_cursor < read_cursor and write_end_cursor >= read_cursor) or (unwrapped_write_end_cursor > queue_size and write_end_cursor >= read_cursor) then");
		self.indent();
		self.push_line("local new_queue_size = queue_size * 2");
		self.push_line("local new_arguments = table.create(new_queue_size)");
		self.push_line("local new_write_cursor");

		self.push_line("if write_cursor >= read_cursor then");
		self.indent();
		self.push_line("table.move(arguments, read_cursor, write_cursor, 1, new_arguments)");
		self.push_line("new_write_cursor = write_cursor - read_cursor + 1");
		self.dedent();
		self.push_line("else");
		self.indent();
		self.push_line("table.move(arguments, read_cursor, queue_size, 1, new_arguments)");
		self.push_line("table.move(arguments, 1, write_cursor, (queue_size - read_cursor) + 1, new_arguments)");
		self.push_line("new_write_cursor = write_cursor + (queue_size - read_cursor) + 1");
		self.dedent();
		self.push_line("end");

		self.push_line("queue.arguments = new_arguments");
		self.push_line("queue.queue_size = new_queue_size");
		self.push_line("queue.read_cursor = 1");
		self.push_line("queue.write_cursor = new_write_cursor");
		for (index, argument) in arguments.iter().enumerate() {
			if index > 0 {
				self.push_line(&format!(
					"new_arguments[((new_write_cursor + {index} - 1) % new_queue_size) + 1] = {argument}"
				));
			} else {
				self.push_line(&format!("new_arguments[new_write_cursor] = {argument}"));
			}
		}
		if returns_length == 1 {
			self.push_line("queue.write_cursor = (write_cursor % new_queue_size) + 1");
		} else {
			self.push_line(&format!(
				"queue.write_cursor = ((write_cursor + {}) % new_queue_size) + 1",
				returns_length - 1
			));
		}
		self.dedent();
		self.push_line("else");
		self.indent();
		for (index, argument) in arguments.iter().enumerate() {
			if index > 0 {
				self.push_line(&format!(
					"arguments[((write_cursor + {index} - 1) % queue_size) + 1] = {argument}"
				));
			} else {
				self.push_line(&format!("arguments[write_cursor] = {argument}"));
			}
		}
		if returns_length == 1 {
			self.push_line("queue.write_cursor = (write_cursor % queue_size) + 1");
		} else {
			self.push_line(&format!(
				"queue.write_cursor = ((write_cursor + {}) % queue_size) + 1",
				returns_length - 1
			));
		}
		self.dedent();
		self.push_line("end");
	}

	fn push_reliable_callback(&mut self, first: bool, ev: &EvDecl) {
		let id = ev.id;

		self.push_indent();

		if first {
			self.push("if ");
		} else {
			self.push("elseif ");
		}

		// push_line is not used here as indent was pushed above
		// and we don't want to push it twice, especially after
		// the if/elseif
		self.push(&format!("id == {id} then"));
		self.push("\n");

		self.indent();

		let values = self.get_values(ev.data.len());

		self.push_line(&format!("local {values}"));

		if !ev.data.is_empty() {
			let statements = &des::generate(
				ev.data.iter().map(|parameter| &parameter.ty),
				&get_unnamed_values("value", ev.data.len()),
				true,
				&mut self.var_occurrences,
			);
			self.push_stmts(statements);
		}

		match ev.call {
			EvCall::SingleSync | EvCall::SingleAsync => self.push_line(&format!("if reliable_events[{id}] then")),
			EvCall::ManySync | EvCall::ManyAsync => self.push_line(&format!("if reliable_events[{id}][1] then")),
			EvCall::Polling => self.push_polling_event(ev),
		}

		self.indent();

		if ev.call == EvCall::ManySync || ev.call == EvCall::ManyAsync {
			self.push_line(&format!("for _, cb in reliable_events[{id}] do"));
			self.indent();
		}

		match ev.call {
			EvCall::SingleSync => self.push_line(&format!("reliable_events[{id}]({values})")),
			EvCall::SingleAsync => self.push_line(&format!("task.spawn(reliable_events[{id}], {values})")),
			EvCall::ManySync => self.push_line(&format!("cb({values})")),
			EvCall::ManyAsync => self.push_line(&format!("task.spawn(cb, {values})")),
			EvCall::Polling => (),
		}

		if ev.call == EvCall::ManySync || ev.call == EvCall::ManyAsync {
			self.dedent();
			self.push_line("end");
		}

		self.dedent();

		if ev.call != EvCall::Polling {
			self.push_line("else");
			self.indent();

			if !ev.data.is_empty() {
				if ev.data.len() > 1 {
					self.push_line(&format!("table.insert(reliable_event_queue[{id}], {{ {values} }})"));
				} else {
					self.push_line(&format!("table.insert(reliable_event_queue[{id}], value)"));
				}

				self.push_line(&format!("if #reliable_event_queue[{id}] > 64 then"));
			} else {
				self.push_line(&format!("reliable_event_queue[{id}] += 1"));
				self.push_line(&format!("if reliable_event_queue[{id}] > 16 then"));
			}

			self.indent();
			self.push_indent();

			self.push("warn(`[ZAP] {");

			if !ev.data.is_empty() {
				self.push("#")
			}

			self.push(&format!(
				"reliable_event_queue[{id}]}} events in queue for {}. Did you forget to attach a listener?`)\n",
				ev.name
			));

			self.dedent();
			self.push_line("end");

			self.dedent();
			self.push_line("end");
		}

		self.dedent();
	}

	fn push_fn_callback(&mut self, first: bool, fndecl: &FnDecl) {
		let client_id = fndecl.client_id;

		self.push_indent();

		if first {
			self.push("if ");
		} else {
			self.push("elseif ");
		}

		// push_line is not used here as indent was pushed above
		// and we don't want to push it twice, especially after
		// the if/elseif
		self.push(&format!("id == {client_id} then"));
		self.push("\n");

		self.indent();

		self.push_line("local call_id = buffer.readu8(incoming_buff, read(1))");

		let values = self.get_values(fndecl.rets.as_ref().map_or(0, |x| x.len()));

		self.push_line(&format!("local {values}"));

		if let Some(data) = &fndecl.rets {
			let statements = &des::generate(
				data,
				&get_unnamed_values("value", data.len()),
				true,
				&mut self.var_occurrences,
			);
			self.push_stmts(statements);
		}

		self.push_line(&format!("local thread = reliable_event_queue[{client_id}][call_id]"));
		self.push_line(
			"-- When using actors it's possible for multiple Zap clients to exist, but only one called the Zap remote function.",
		);
		self.push_line("if thread then");
		self.indent();
		match self.config.yield_type {
			YieldType::Yield | YieldType::Future => {
				self.push_line(&format!("task.spawn(thread, {values})"));
			}
			YieldType::Promise => {
				self.push_line(&format!("thread({values})"));
			}
		}
		self.dedent();
		self.push_line("end");

		self.push_line(&format!("reliable_event_queue[{client_id}][call_id] = nil"));

		self.dedent();
	}

	fn push_iter(&mut self, ev: &EvDecl) {
		let iter = self.config.casing.with("Iter", "iter", "iter");
		let id = ev.id;
		self.push_indent();
		self.push(&format!(
			"{iter} = {}[{id}].iterator :: () -> (() -> (number",
			polling_queues_name(ev)
		));
		if !ev.data.is_empty() {
			for argument in ev.data.iter() {
				self.push(", ");
				self.push_ty(&argument.ty);
			}
		}
		self.push(")),\n");
	}

	fn push_reliable_footer(&mut self) {
		self.push_line("else");
		self.indent();
		self.push_line("error(\"Unknown event id\")");
		self.dedent();
		self.push_line("end");
		self.dedent();
		self.push_line("end");
		self.dedent();
		self.push_line("end)");
	}

	fn push_reliable(&mut self) {
		let mut first = true;

		for evdecl in self
			.config
			.evdecls()
			.iter()
			.filter(|evdecl| evdecl.from == EvSource::Server && evdecl.evty == EvType::Reliable)
		{
			if first {
				self.push_reliable_header();
			}
			self.push_reliable_callback(first, evdecl);
			first = false;
		}

		for fndecl in self.config.fndecls().iter() {
			if first {
				self.push_reliable_header();
			}
			self.push_fn_callback(first, fndecl);
			first = false;
		}

		if !first {
			self.push_reliable_footer();
		}
	}

	fn push_unreliable_callback(&mut self, ev: &EvDecl) {
		let id = ev.id;

		self.push_line(&format!(
			"unreliable[{}].OnClientEvent:Connect(function(buff, inst)",
			id + 1
		));
		self.indent();
		self.push_line("incoming_buff = buff");
		self.push_line("incoming_inst = inst");
		self.push_line("incoming_read = 0");
		self.push_line("incoming_ipos = 0");

		if ev.evty == EvType::Unreliable(true) {
			self.push_line(&format!(
				"local order_id = buffer.read{UNRELIABLE_ORDER_NUMTY}(incoming_buff, read({}))",
				UNRELIABLE_ORDER_NUMTY.size()
			));
			let last = format!("incoming_ids[{id}]");
			self.push_line(&format!(
				"if {last} and order_id <= {last} and {last} - order_id < {} then",
				(UNRELIABLE_ORDER_NUMTY.max() / 2.0).floor(),
			));
			self.indent();
			self.push_line("return");
			self.dedent();
			self.push_line("end");
			self.push_line(&format!("{last} = order_id"));
		}

		let values = self.get_values(ev.data.len());

		self.push_line(&format!("local {values}"));

		if !ev.data.is_empty() {
			let statements = &des::generate(
				ev.data.iter().map(|parameter| &parameter.ty),
				&get_unnamed_values("value", ev.data.len()),
				self.config.write_checks,
				&mut self.var_occurrences,
			);
			self.push_stmts(statements);
		}

		if ev.call == EvCall::Polling {
			self.push_polling_event(ev)
		} else {
			if ev.call == EvCall::SingleSync || ev.call == EvCall::SingleAsync {
				self.push_line(&format!("if unreliable_events[{id}] then"));
			} else {
				self.push_line(&format!("if unreliable_events[{id}][1] then"));
			}

			self.indent();

			if ev.call == EvCall::ManySync || ev.call == EvCall::ManyAsync {
				self.push_line(&format!("for _, cb in unreliable_events[{id}] do"));
				self.indent();
			}

			match ev.call {
				EvCall::SingleSync => self.push_line(&format!("unreliable_events[{id}]({values})")),
				EvCall::SingleAsync => self.push_line(&format!("task.spawn(unreliable_events[{id}], {values})")),
				EvCall::ManySync => self.push_line(&format!("cb({values})")),
				EvCall::ManyAsync => self.push_line(&format!("task.spawn(cb, {values})")),
				EvCall::Polling => (),
			}

			if ev.call == EvCall::ManySync || ev.call == EvCall::ManyAsync {
				self.dedent();
				self.push_line("end");
			}

			self.dedent();
			self.push_line("else");
			self.indent();

			if !ev.data.is_empty() {
				if ev.data.len() > 1 {
					self.push_line(&format!("table.insert(unreliable_event_queue[{id}], {{ {values} }})"));
				} else {
					self.push_line(&format!("table.insert(unreliable_event_queue[{id}], value)"));
				}

				self.push_line(&format!("if #unreliable_event_queue[{id}] > 64 then"));
			} else {
				self.push_line(&format!("unreliable_event_queue[{id}] += 1"));
				self.push_line(&format!("if unreliable_event_queue[{id}] > 16 then"));
			}

			self.indent();
			self.push_indent();

			self.push("warn(`[ZAP] {");

			if !ev.data.is_empty() {
				self.push("#")
			}

			self.push(&format!(
				"unreliable_event_queue[{id}]}} events in queue for {}. Did you forget to attach a listener?`)\n",
				ev.name
			));

			self.dedent();
			self.push_line("end");

			self.dedent();
			self.push_line("end");
		}

		self.dedent();
		self.push_line("end)");
	}

	fn push_unreliable(&mut self) {
		for ev in self
			.config
			.evdecls()
			.iter()
			.filter(|ev_decl| ev_decl.from == EvSource::Server && matches!(ev_decl.evty, EvType::Unreliable(_)))
		{
			self.push_unreliable_callback(ev);
		}
	}

	fn push_callback_lists(&mut self) {
		let client_reliable_count = self.config.client_reliable_count();
		let client_unreliable_count = self.config.client_unreliable_count();

		if client_reliable_count > 0 {
			self.push_line(&format!(
				"local reliable_events = table.create({client_reliable_count})"
			));
			self.push_line(&format!(
				"local reliable_event_queue: {{ [number]: {{ any }} }} = table.create({client_reliable_count})"
			));
		}

		if client_unreliable_count > 0 {
			self.push_line(&format!(
				"local unreliable_events = table.create({client_unreliable_count})"
			));
			self.push_line(&format!(
				"local unreliable_event_queue: {{ [number]: {{ any }} }} = table.create({client_unreliable_count})"
			));
		}

		if !self.config.fndecls().is_empty() {
			self.push_line("local function_call_id = 0");

			if self.config.typescript && self.config.yield_type == YieldType::Promise {
				self.push_line("local Promise = _G[script].Promise")
			} else if !self.config.async_lib.is_empty() {
				self.push_line(&format!("local {} = {}", self.config.yield_type, self.config.async_lib))
			}
		}

		for evdecl in self
			.config
			.evdecls()
			.iter()
			.filter(|ev_decl| ev_decl.from == EvSource::Server)
		{
			let id = evdecl.id;

			if evdecl.call == EvCall::ManyAsync || evdecl.call == EvCall::ManySync {
				self.push_line(&format!("{}[{}] = {{}}", events_table_name(evdecl), id));
			}

			if !evdecl.data.is_empty() {
				self.push_line(&format!("{}[{id}] = {{}}", event_queue_table_name(evdecl)));
			} else {
				self.push_line(&format!("{}[{id}] = 0", event_queue_table_name(evdecl)));
			}
		}

		for fndecl in self.config.fndecls().iter() {
			self.push_line(&format!(
				"reliable_event_queue[{}] = table.create(255)",
				fndecl.client_id
			));
		}
	}

	fn push_write_event_id(&mut self, id: usize) {
		let num_ty = self.config.server_reliable_ty();

		self.push_line(&format!("alloc({})", num_ty.size()));
		self.push_line(&format!("buffer.write{num_ty}(outgoing_buff, outgoing_apos, {id})"));
	}

	fn push_write_evdecl_event_id(&mut self, ev: &EvDecl) {
		if ev.evty == EvType::Reliable {
			self.push_write_event_id(ev.id);
		}
	}

	fn push_value_parameters(&mut self, parameters: &[Parameter]) {
		for (i, parameter) in parameters.iter().enumerate() {
			if i > 0 {
				self.push(", ");
			}

			if let Some(name) = parameter.name {
				self.push(&format!("{name}: "));
			} else {
				let value = format!(
					"{}{}",
					self.config.casing.with("Value", "value", "value"),
					if i == 0 { "".to_string() } else { (i + 1).to_string() }
				);

				self.push(&format!("{value}: "));
			}

			self.push_ty(&parameter.ty);
		}
	}

	fn push_return_fire(&mut self, ev: &EvDecl) {
		let fire = self.config.casing.with("Fire", "fire", "fire");
		let value = self.config.casing.with("Value", "value", "value");

		self.push_indent();
		self.push(&format!("{fire} = function("));

		if !ev.data.is_empty() {
			self.push_value_parameters(&ev.data);
		}

		self.push(")\n");
		self.indent();

		if let EvType::Unreliable(ordered) = ev.evty {
			let id = ev.id;
			if ordered {
				self.push_line(&format!(
					"local order_id = ((outgoing_ids[{id}] or -1) + 1) % {}",
					UNRELIABLE_ORDER_NUMTY.max() + 1.0
				));
				self.push_line(&format!("outgoing_ids[{id}] = order_id"));
			}
			self.push_line("local saved = save()");
			self.push_line("load_empty()");
			if ordered {
				self.push_line(&format!("alloc({})", UNRELIABLE_ORDER_NUMTY.size()));
				self.push_line(&format!(
					"buffer.write{UNRELIABLE_ORDER_NUMTY}(outgoing_buff, outgoing_apos, order_id)"
				));
			}
		}

		self.push_write_evdecl_event_id(ev);

		if !ev.data.is_empty() {
			let statements = &ser::generate(
				ev.data.iter().map(|parameter| &parameter.ty),
				&get_named_values(value, &ev.data),
				self.config.write_checks,
				&mut self.var_occurrences,
			);
			self.push_stmts(statements);
		}

		if matches!(ev.evty, EvType::Unreliable(_)) {
			self.push_line("local buff = buffer.create(outgoing_used)");
			self.push_line("buffer.copy(buff, 0, outgoing_buff, 0, outgoing_used)");
			self.push_line(&format!("unreliable[{}]:FireServer(buff, outgoing_inst)", ev.id + 1));
			self.push_line("load(saved)");
		}

		self.dedent();
		self.push_line("end,");
	}

	fn push_queued_value(&mut self, parameters: &[Parameter]) {
		if parameters.len() > 1 {
			self.push("unpack(value)");
		} else {
			self.push("value");
		}
	}

	fn push_return_setcallback(&mut self, ev: &EvDecl) {
		let id = ev.id;

		let set_callback = self.config.casing.with("SetCallback", "setCallback", "set_callback");
		let callback = self.config.casing.with("Callback", "callback", "callback");

		self.push_indent();
		self.push(&format!("{set_callback} = function({callback}: ("));

		if !ev.data.is_empty() {
			self.push_value_parameters(&ev.data);
		}

		self.push(") -> ()): () -> ()\n");
		self.indent();

		self.push_line(&format!("{}[{id}] = {callback}", events_table_name(ev)));

		let event_queue_name = event_queue_table_name(ev);

		if !ev.data.is_empty() {
			self.push_line(&format!("for _, value in {event_queue_name}[{id}] do"));
			self.indent();

			if ev.call == EvCall::SingleSync {
				self.push_indent();
				self.push(&format!("{callback}("));
				self.push_queued_value(&ev.data);
				self.push_line(")\n");
			} else {
				self.push_indent();
				self.push(&format!("task.spawn({callback}, "));
				self.push_queued_value(&ev.data);
				self.push(")\n");
			}

			self.dedent();
			self.push_line("end");

			self.push_line(&format!("{event_queue_name}[{id}] = {{}}"));
		} else {
			self.push_line(&format!("for _ = 1, {event_queue_name}[{id}] do"));
			self.indent();

			if ev.call == EvCall::SingleSync {
				self.push_line(&format!("{callback}()"))
			} else {
				self.push_line(&format!("task.spawn({callback})"))
			}

			self.dedent();
			self.push_line("end");

			self.push_line(&format!("{event_queue_name}[{id}] = 0"));
		}

		self.push_line("return function()");
		self.indent();

		self.push_line(&format!("{}[{id}] = nil", events_table_name(ev)));

		self.dedent();
		self.push_line("end");

		self.dedent();
		self.push_line("end,");
	}

	fn push_return_on(&mut self, ev: &EvDecl) {
		let id = ev.id;

		let on = self.config.casing.with("On", "on", "on");
		let callback = self.config.casing.with("Callback", "callback", "callback");

		self.push_indent();
		self.push(&format!("{on} = function({callback}: ("));

		if !ev.data.is_empty() {
			self.push_value_parameters(&ev.data);
		}

		self.push(") -> ())\n");
		self.indent();

		let events_table_name = events_table_name(ev);
		let event_queue_name = event_queue_table_name(ev);

		self.push_line(&format!("table.insert({events_table_name}[{id}], {callback})"));

		if !ev.data.is_empty() {
			self.push_line(&format!("for _, value in {event_queue_name}[{id}] do"));
			self.indent();

			if ev.call == EvCall::ManySync {
				self.push_indent();
				self.push(&format!("{callback}("));
				self.push_queued_value(&ev.data);
				self.push_line(")\n");
			} else {
				self.push_indent();
				self.push(&format!("task.spawn({callback}, "));
				self.push_queued_value(&ev.data);
				self.push(")\n");
			}

			self.dedent();
			self.push_line("end");

			self.push_line(&format!("{event_queue_name}[{id}] = {{}}"));
		} else {
			self.push_line(&format!("for _ = 1, {event_queue_name}[{id}] do"));
			self.indent();

			if ev.call == EvCall::ManySync {
				self.push_line(&format!("{callback}()"))
			} else {
				self.push_line(&format!("task.spawn({callback})"))
			}

			self.dedent();
			self.push_line("end");

			self.push_line(&format!("{event_queue_name}[{id}] = 0"));
		}

		self.push_line("return function()");
		self.indent();

		self.push_line(&format!(
			"table.remove({events_table_name}[{id}], table.find({events_table_name}[{id}], {callback}))"
		));

		self.dedent();
		self.push_line("end");

		self.dedent();
		self.push_line("end,");
	}

	fn push_polling(&mut self) {
		let filtered_evdecls = self
			.config
			.evdecls()
			.into_iter()
			.filter(|evdecl| evdecl.from == EvSource::Server && evdecl.call == EvCall::Polling)
			.collect::<Vec<&EvDecl>>();

		if !filtered_evdecls.is_empty() {
			self.push("\n");
		}

		for evdecl in filtered_evdecls {
			let id = evdecl.id;
			let mut return_names: Vec<String> = vec![];
			if evdecl.data.is_empty() {
				return_names.push(String::from("value"));
			}
			for (index, parameter) in evdecl.data.iter().enumerate() {
				return_names.push(format!(
					"value_{}{}",
					index + 1,
					if let Some(name) = parameter.name {
						format!("_{name}")
					} else {
						String::from("")
					}
				));
			}
			let arguments_size = return_names.len().max(1);

			self.push_line(&format!("{}[{id}] = {{", polling_queues_name(evdecl)));
			self.indent();

			self.push_line(&format!(
				"arguments = table.create({}),",
				max(arguments_size * super::INITIAL_POLLING_EVENT_CAPACITY, 1)
			));
			self.push_line(&format!(
				"queue_size = {},",
				max(arguments_size * super::INITIAL_POLLING_EVENT_CAPACITY, 1)
			));
			self.push_line("read_cursor = 1,");
			self.push_line("write_cursor = 1,");
			self.push_line("iterator = function()");
			self.indent();

			self.push_line(&format!("local queue = {}[{id}]", polling_queues_name(evdecl)));
			self.push_line("local index = 0");
			self.push_line("return function()");
			self.indent();

			self.push_line("index += 1");

			self.push_line("if queue.read_cursor == queue.write_cursor then");
			self.indent();

			self.push_line("return nil");

			// end `if queue.read_cursor == queue.write_cursor then`
			self.dedent();
			self.push_line("end");

			// `queue` fields could be changed if the user yields while polling and more events are written that force the queue to grow.
			// That's why these are defined inside the generator.
			self.push_line("local arguments = queue.arguments");
			self.push_line("local read_cursor = queue.read_cursor");
			self.push_line("local queue_size = queue.queue_size");
			self.push_indent();
			self.push("local ");
			self.push(&return_names.join(", "));
			self.push(" = ");
			for argument_index in 0..return_names.len() {
				if argument_index > 0 {
					self.push(", ");
				}
				self.push(&format!(
					"arguments[{}]",
					if argument_index == 0 {
						String::from("read_cursor")
					} else if argument_index == 1 {
						String::from("(read_cursor % queue_size) + 1")
					} else {
						format!("((read_cursor + {}) % queue_size) + 1", argument_index - 1)
					}
				));
			}
			self.push("\n");
			if arguments_size == 1 {
				self.push_line("queue.read_cursor = (queue.read_cursor % queue.queue_size) + 1");
			} else {
				self.push_line(&format!(
					"queue.read_cursor = ((queue.read_cursor + {}) % queue.queue_size) + 1",
					arguments_size - 1
				));
			}
			self.push_line(&format!("return index, {}", return_names.join(", ")));

			// dedent generator
			self.dedent();
			self.push_line("end");

			// dedent iterator
			self.dedent();
			self.push_line("end,");

			// dedent queue definition
			self.dedent();
			self.push_line("}");
		}
		self.push_line("table.freeze(polling_queues_reliable)");
		self.push_line("table.freeze(polling_queues_unreliable)\n");
	}

	fn push_return(&mut self) {
		self.push_line("local returns = {");
		self.indent();

		let send_events = self.config.casing.with("SendEvents", "sendEvents", "send_events");

		self.push_line(&format!("{send_events} = {send_events},"));

		let call = self.config.casing.with("Call", "call", "call");
		let value = self.config.casing.with("Value", "value", "value");

		self.config.traverse_namespaces(
			self,
			|this, diff| {
				for _ in 0..diff {
					this.dedent();
					this.push_line("},");
				}
			},
			|this, path, entry| {
				let name = path.last().unwrap();
				this.push_line(&format!("{name} = {{"));
				this.indent();

				match entry {
					NamespaceEntry::EvDecl(evdecl) if evdecl.from == EvSource::Client => {
						this.push_return_fire(evdecl);

						this.dedent();
						this.push_line("},");
					}
					NamespaceEntry::EvDecl(evdecl) => {
						match evdecl.call {
							EvCall::SingleSync | EvCall::SingleAsync => this.push_return_setcallback(evdecl),
							EvCall::ManySync | EvCall::ManyAsync => this.push_return_on(evdecl),
							EvCall::Polling => this.push_iter(evdecl),
						}

						this.dedent();
						this.push_line("},");
					}
					NamespaceEntry::FnDecl(fndecl) => {
						let client_id = fndecl.client_id;

						this.push_indent();
						this.push(&format!("{call} = function("));

						if !fndecl.args.is_empty() {
							this.push_value_parameters(&fndecl.args);
						}

						this.push(")");

						if let Some(types) = &fndecl.rets {
							match this.config.yield_type {
								YieldType::Future => {
									this.push(": Future.Future<(");

									for (i, ty) in types.iter().enumerate() {
										if i > 0 {
											this.push(", ");
										}
										this.push_ty(ty);
									}

									this.push(")>");
								}
								YieldType::Yield => {
									this.push(": (");
									for (i, ty) in types.iter().enumerate() {
										if i > 0 {
											this.push(", ");
										}
										this.push_ty(ty);
									}
									this.push(")");
								}
								_ => (),
							}
						}

						this.push("\n");
						this.indent();

						this.push_write_event_id(fndecl.server_id);

						this.push_line("function_call_id += 1");

						this.push_line("function_call_id %= 256");

						this.push_line(&format!("if reliable_event_queue[{client_id}][function_call_id] then"));
						this.indent();

						this.push_line("function_call_id -= 1");
						this.push_line(
							"error(\"Zap has more than 256 calls awaiting a response, and therefore this packet has been dropped\")",
						);

						this.dedent();
						this.push_line("end");

						this.push_line("alloc(1)");
						this.push_line("buffer.writeu8(outgoing_buff, outgoing_apos, function_call_id)");

						if !fndecl.args.is_empty() {
							let statements = &ser::generate(
								fndecl.args.iter().map(|parameter| &parameter.ty),
								&get_named_values(value, &fndecl.args),
								this.config.write_checks,
								&mut this.var_occurrences,
							);
							this.push_stmts(statements);
						}

						match self.config.yield_type {
							YieldType::Yield => {
								this.push_line(&format!(
									"reliable_event_queue[{client_id}][function_call_id] = coroutine.running()"
								));
								this.push_line("return coroutine.yield()");
							}
							YieldType::Future => {
								this.push_line("return Future.new(function()");
								this.indent();

								this.push_line(&format!(
									"reliable_event_queue[{client_id}][function_call_id] = coroutine.running()"
								));
								this.push_line("return coroutine.yield()");

								this.dedent();
								this.push_line("end)");
							}
							YieldType::Promise => {
								this.push_line("return Promise.new(function(resolve)");
								this.indent();

								this.push_line(&format!(
									"reliable_event_queue[{client_id}][function_call_id] = resolve"
								));

								this.dedent();
								this.push_line("end)");
							}
						}

						this.dedent();
						this.push_line("end,");

						this.dedent();
						this.push_line("},");
					}
					NamespaceEntry::Ns(..) => {}
				}
			},
		);

		self.dedent();
		self.push_line("}");

		self.push_line("type Events = typeof(returns)");
		self.push_line("return returns");
	}

	pub fn push_remotes(&mut self) {
		self.push_line(&format!(
			"local remotes = ReplicatedStorage:WaitForChild(\"{}\")",
			self.config.remote_folder
		));
		self.push("\n");

		self.push_line(&format!(
			"local reliable = remotes:WaitForChild(\"{}_RELIABLE\")",
			self.config.remote_scope
		));
		self.push_line(&format!(
			"assert(reliable:IsA(\"RemoteEvent\"), \"Expected {}_RELIABLE to be a RemoteEvent\")",
			self.config.remote_scope
		));
		self.push("\n");

		let unreliable_count = max(
			self.config.client_unreliable_count(),
			self.config.server_unreliable_count(),
		);

		if unreliable_count > 0 {
			self.push_indent();
			self.push("local unreliable = { ");

			for id in 0..unreliable_count {
				if id != 0 {
					self.push(", ")
				}

				self.push(&format!(
					"remotes:WaitForChild(\"{}_UNRELIABLE_{id}\")",
					self.config.remote_scope
				));
			}

			self.push(" }\n");

			for id in 0..unreliable_count {
				self.push_line(&format!("assert(unreliable[{}]:IsA(\"UnreliableRemoteEvent\"), \"Expected {}_UNRELIABLE_{id} to be an UnreliableRemoteEvent\")", id + 1, self.config.remote_scope));
			}
		}
	}

	pub fn push_check_server(&mut self) {
		self.push_line("if RunService:IsServer() then");
		self.indent();
		self.push_line("error(\"Cannot use the client module on the server!\")");
		self.dedent();
		self.push_line("end");
	}

	pub fn output(mut self) -> String {
		self.push_file_header("Client");

		if self.config.namespaces.is_empty() {
			self.push_line("return {}");
			return self.buf;
		};

		self.push(include_str!("base.luau"));

		self.push_studio();

		self.push_check_server();

		self.push_remotes();

		self.push_tydecls();

		self.push_event_loop();

		self.push_callback_lists();

		self.push_reliable();

		self.push_unreliable();

		self.push_polling();

		self.push_return();

		self.buf
	}
}

pub fn code<'src>(config: &'src Config<'src>) -> String {
	ClientOutput::new(config).output()
}
