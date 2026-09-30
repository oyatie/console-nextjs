-- no-transaction
-- Validate messenger_threads_work_order_direct_check after 0122 installed it as NOT VALID.
ALTER TABLE messenger_threads VALIDATE CONSTRAINT messenger_threads_work_order_direct_check;
